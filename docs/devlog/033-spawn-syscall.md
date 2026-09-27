# Day 33 — Ring 3 can launch ring 3: the spawn syscall

Every program so far was launched by the shell. Today a program can launch *another program itself*, with no kernel-side shell involved — the first real step toward a user-space init/shell.

## The syscall

`spawn(name_ptr, name_len, args_ptr, args_len) -> pid, or -1` (rax = 11). Name resolution is exactly what the shell's `run`/`exec` already do: try a built-in first, else read `<name>` as an AXE1 file from the filesystem. On success it builds a brand new address space (Stage 31's `spawn_bytes`) and starts a kernel thread for it — the child is a first-class process, not a sub-thread of the caller.

## Demo: `launcher`

Spawns `hello` (no args) and `cat` with argument `motd`, sleeps briefly, then reports and exits — while its children keep running on their own schedule:

```
launcher: spawning two child programs via the spawn syscall...
[user] pid 2 'hello' -> ring 3 ...
Hello from ring 3! ...
[user] pid 3 'cat' -> ring 3 ...
Welcome to AetherOS. ...
launcher: done (children run independently - see ps while they're alive).
```

Also verified: `install` + `exec launcher.axe` — a filesystem-loaded program spawning both a built-in (`hello`) and another filesystem lookup (`cat`) — and the whole thing under `-cpu max` (NX/SMEP/SMAP on) alongside `producer`/`consumer` (Stage 32) running at the same time, with no regressions.

## Stated plainly: no process tree yet

`spawn` returns a pid but there is no `wait`/reap syscall and no parent-child relationship recorded — a spawned process is exactly as independent as one the shell started, just started from ring 3 instead. That (and signals) are the natural next step toward a real user-space shell.
