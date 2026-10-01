"""Run AetherOS scheduling experiments unattended and collect the results.

How it works
------------
1. Writes an `autorun` script (a list of `bench ...` lines) into a fresh
   data-disk image, in the kernel's own AFS1 on-disk format.
2. Boots the kernel in QEMU with that disk attached. At boot the kernel
   restores its filesystem from the disk, finds `autorun`, runs every
   benchmark in it, and powers the machine off.
3. Parses the RESULT / SUMMARY / LAT lines the kernel printed on its serial
   port into CSV files.

Usage
-----
    python experiments/aetherbench.py --script experiments/plans/smoke.txt --out results/smoke

Timing mode: by default QEMU runs with `-icount shift=3,sleep=off`, so the
guest's clock advances by instruction count (8 ns per instruction) rather than
host wall-clock time. That makes results independent of what else the host is
doing, and makes a given (workload, policy, seed) reproducible. Pass
`--realtime` to drop -icount (results then include host noise).
"""

import argparse
import csv
import os
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
KERNEL = os.path.dirname(HERE)
DEFAULT_BIN = os.path.join(KERNEL, "target", "x86_64-unknown-none", "debug",
                           "bootimage-aetheros_kernel.bin")
DEFAULT_QEMU = r"C:\Program Files\qemu\qemu-system-x86_64.exe"

RESULT_FIELDS = ["run", "workload", "policy", "q", "hints", "seed", "pid", "prog", "hint",
                 "status", "response_us", "turnaround_us", "cpu_us", "wait_us", "switches",
                 "preempts", "blocks", "lat_n", "lat_p50", "lat_p95", "lat_p99", "lat_max"]
SUMMARY_FIELDS = ["run", "workload", "policy", "q", "hints", "seed", "nprocs", "makespan_us",
                  "ctx_switches", "sched_us"]


# ---------------------------------------------------------------- AFS1 image

def fnv1a32(data: bytes) -> int:
    h = 0x811C9DC5
    for b in data:
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def build_data_image(path: str, files: dict, size: int = 2 * 1024 * 1024) -> None:
    """Write a disk image holding `files` ({name: text}) in the kernel's AFS1
    format (see src/persist.rs)."""
    blob = b""
    for name, text in files.items():
        n, c = name.encode("utf-8"), text.encode("utf-8")
        blob += struct.pack("<HI", len(n), len(c)) + n + c
    header = b"AFS1" + struct.pack("<III", len(blob), fnv1a32(blob), len(files))
    sectors = (len(blob) + 511) // 512
    if 1 + sectors > 2048:
        raise ValueError("script too large for AFS1 (max 2047 data sectors)")
    img = bytearray(size)
    img[0:len(header)] = header
    img[512:512 + len(blob)] = blob
    with open(path, "wb") as f:
        f.write(img)


# ---------------------------------------------------------------- running

def elf_symbols(path):
    """Function symbols of a 64-bit little-endian ELF: sorted [(addr, size, name)]."""
    try:
        with open(path, "rb") as f:
            data = f.read()
    except OSError:
        return []
    if data[:4] != b"\x7fELF":
        return []
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum = struct.unpack_from("<HH", data, 0x3A)
    sections = []
    for i in range(shnum):
        base = shoff + i * shentsize
        _, sh_type, _, _, off, size, link, _, _, entsize = struct.unpack_from("<IIQQQQIIQQ", data, base)
        sections.append((sh_type, off, size, link, entsize))
    syms = []
    for sh_type, off, size, link, entsize in sections:
        if sh_type != 2 or not entsize:  # SHT_SYMTAB
            continue
        str_off = sections[link][1]
        for j in range(size // entsize):
            name_off, info, _, _, value, sz = struct.unpack_from("<IBBHQQ", data, off + j * entsize)
            if info & 0xF != 2 or not value:  # STT_FUNC
                continue
            end = data.index(b"\0", str_off + name_off)
            syms.append((value, sz, data[str_off + name_off:end].decode("utf-8", "replace")))
    syms.sort()
    return syms


def symbolize(addr, syms):
    best = None
    for a, sz, name in syms:
        if a > addr:
            break
        best = (a, sz, name)
    if best and (best[1] == 0 or addr < best[0] + best[1]):
        return f"{best[2]}+{addr - best[0]:#x}"
    return "?"


def monitor_dump(port, kernel_elf):
    """Ask a stuck QEMU what its CPU is doing (registers, code at RIP, stack)."""
    import re
    import socket
    out = []
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=5)
        s.settimeout(2)
        def cmd(c):
            s.sendall((c + "\n").encode())
            time.sleep(0.7)
            buf = b""
            try:
                while True:
                    chunk = s.recv(65536)
                    if not chunk:
                        break
                    buf += chunk
            except OSError:
                pass
            return buf.decode("utf-8", "replace")
        cmd("")
        regs = cmd("info registers")
        out.append(regs)
        out.append(cmd("x/12i $pc"))
        out.append(cmd("x/48gx $rsp"))
        s.close()
        syms = elf_symbols(kernel_elf)
        m = re.search(r"RIP=([0-9a-f]+)", regs)
        if m and syms:
            out.insert(0, f"RIP is in: {symbolize(int(m.group(1), 16), syms)}\n")
            # also symbolize anything on the stack that looks like a kernel code address
            lo, hi = syms[0][0], syms[-1][0] + syms[-1][1]
            frames = []
            for v in re.findall(r"0x([0-9a-f]{16})", out[-1]):
                a = int(v, 16)
                if lo <= a < hi:
                    frames.append(symbolize(a, syms))
            out.insert(1, "kernel code addresses on the stack:\n  " + "\n  ".join(frames) + "\n")
    except OSError as e:
        out.append(f"monitor dump failed: {e}")
    return "\n".join(out)


def run_qemu(qemu, kernel_bin, data_img, serial_path, realtime, timeout, cpu=None,
             stall=120, port=None, stall_path=None):
    args = [qemu,
            "-drive", f"format=raw,file={kernel_bin}",
            "-drive", f"file={data_img},format=raw,if=ide,index=1",
            "-display", "none",
            "-serial", f"file:{serial_path}",
            "-monitor", f"tcp:127.0.0.1:{port},server,nowait" if port else "none",
            "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-no-reboot", "-m", "256M"]
    if not realtime:
        args += ["-icount", "shift=3,sleep=off"]
    if cpu:
        args += ["-cpu", cpu]
    t0 = time.time()
    err_path = os.path.join(os.path.dirname(serial_path) or ".", "qemu_stderr.txt")
    err_file = open(err_path, "w")
    p = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=err_file)
    last_size, last_change = -1, time.time()
    while p.poll() is None:
        time.sleep(2)
        try:
            size = os.path.getsize(serial_path)
        except OSError:
            size = 0
        if size != last_size:
            last_size, last_change = size, time.time()
        stalled = time.time() - last_change > stall
        if stalled or time.time() - t0 > timeout:
            if port and stall_path:
                kernel_elf = os.path.splitext(kernel_bin)[0].replace("bootimage-", "")
                with open(stall_path, "w", encoding="utf-8") as f:
                    f.write(("STALLED" if stalled else "TIMEOUT") + f" after {time.time() - t0:.0f}s\n")
                    f.write(monitor_dump(port, kernel_elf))
            p.kill()
            p.wait()
            err_file.close()
            return False, time.time() - t0
    err_file.close()
    return True, time.time() - t0


def parse_serial(text):
    results, summaries, lats = [], [], {}
    errors = []
    for line in text.splitlines():
        line = line.strip()
        # A record can be glued to other output that lacked a newline (e.g. the
        # shell's "aether> " prompt printed at boot), so find the tag anywhere.
        for tag in ("RESULT,", "SUMMARY,", "LAT,"):
            i = line.find(tag)
            if i > 0:
                line = line[i:]
                break
        if line.startswith("RESULT,"):
            parts = line.split(",")[1:]
            if len(parts) == len(RESULT_FIELDS):
                results.append(dict(zip(RESULT_FIELDS, parts)))
            else:
                errors.append(line)
        elif line.startswith("SUMMARY,"):
            parts = line.split(",")[1:]
            if len(parts) == len(SUMMARY_FIELDS):
                summaries.append(dict(zip(SUMMARY_FIELDS, parts)))
            else:
                errors.append(line)
        elif line.startswith("LAT,"):
            _, run, pid, samples = line.split(",", 3)
            lats[(run, pid)] = [int(x) for x in samples.split(";") if x]
        elif "AUTORUN,error" in line or "PANIC" in line or "could not launch" in line:
            errors.append(line)
    return results, summaries, lats, errors


def write_csv(path, rows, fields):
    with open(path, "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fields)
        w.writeheader()
        for r in rows:
            w.writerow(r)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True, help="text file of autorun lines (bench/repeat/echo)")
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--qemu", default=DEFAULT_QEMU)
    ap.add_argument("--realtime", action="store_true", help="no -icount (wall-clock timing)")
    ap.add_argument("--timeout", type=float, default=1800)
    ap.add_argument("--cpu", default=None, help="QEMU -cpu model (e.g. max)")
    ap.add_argument("--files", nargs="*", default=[],
                    help="extra files to put on the data disk (e.g. compiled .axe programs "
                         "for `bench files:...` workloads)")
    ap.add_argument("--stall", type=float, default=120,
                    help="seconds without serial output before the run is declared hung "
                         "(registers + stack are then dumped to stall.txt)")
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    with open(a.script, encoding="utf-8") as f:
        script = f.read()
    if "poweroff" not in script:
        script = script.rstrip("\n") + "\npoweroff\n"

    data_img = os.path.join(a.out, "data.img")
    serial = os.path.join(a.out, "serial.log")
    files = {"autorun": script}
    for path in a.files:
        with open(path, encoding="utf-8") as f:
            files[os.path.basename(path)] = f.read()
    build_data_image(data_img, files)
    if os.path.exists(serial):
        os.remove(serial)

    print(f"booting QEMU ({'realtime' if a.realtime else 'icount'})...", flush=True)
    import socket
    with socket.socket() as probe:       # let the OS pick a port nobody is using
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    finished, secs = run_qemu(a.qemu, a.bin, data_img, serial, a.realtime, a.timeout, a.cpu,
                              stall=a.stall, port=port, stall_path=os.path.join(a.out, "stall.txt"))
    if not os.path.exists(serial):
        err = open(os.path.join(a.out, "qemu_stderr.txt"), encoding="utf-8", errors="replace").read()
        print("QEMU never started the guest. Its error output:\n" + err)
        sys.exit(1)
    with open(serial, encoding="utf-8", errors="replace") as f:
        text = f.read()
    results, summaries, lats, errors = parse_serial(text)

    write_csv(os.path.join(a.out, "results.csv"), results, RESULT_FIELDS)
    write_csv(os.path.join(a.out, "summary.csv"), summaries, SUMMARY_FIELDS)
    with open(os.path.join(a.out, "latency.csv"), "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["run", "pid", "lat_us"])
        for (run, pid), samples in sorted(lats.items(), key=lambda kv: (int(kv[0][0]), int(kv[0][1]))):
            for v in samples:
                w.writerow([run, pid, v])
    with open(os.path.join(a.out, "config.txt"), "w", encoding="utf-8") as f:
        f.write(f"timing={'realtime' if a.realtime else 'icount shift=3,sleep=off'}\n")
        f.write(f"cpu={a.cpu or 'default'}\nbin={a.bin}\n\n{script}")

    print(f"{'finished' if finished else 'TIMED OUT'} in {secs:.1f}s host time: "
          f"{len(summaries)} benchmark runs, {len(results)} process results")
    for e in errors[:10]:
        print("  !!", e)
    if not finished or errors or "AUTORUN,done" not in text:
        print("  (see", serial, ")")
        sys.exit(1)


if __name__ == "__main__":
    main()
