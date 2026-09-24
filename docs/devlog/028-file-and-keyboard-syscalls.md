# Day 28 — User programs get files, arguments and a keyboard

Ring-3 programs could print and sleep. Now they can *do things*: read and write files, receive arguments, and read keystrokes.

## New syscalls

| rax | name | args | notes |
|---|---|---|---|
| 5 | readfile | name ptr/len, buf ptr/len | copies file into the program's **stack** buffer |
| 6 | getargs | buf ptr/len | the text after the program name |
| 7 | readkey | - | blocks until a key arrives |
| 8 | writefile | name ptr/len, data ptr/len | text only (UTF-8), max 2 KiB |

A 4th argument arrives in `r10` (as on Linux); the entry stub moves it to `r8` for the C ABI.

## Validation is per direction

- **Kernel reads user memory** (names, data): pointer must lie fully inside the program's code or stack pages.
- **Kernel writes user memory** (`readfile`, `getargs` buffers): must lie inside the *stack* only — the code pages are read-only to the program, and a kernel write to them would be a way around that.
- File names are capped at 128 bytes and must be UTF-8.

## Keyboard routing

Keys normally go to the shell. While a program is *blocked inside `readkey`* the keyboard interrupt handler hands keys to it (`offer_key`) instead; otherwise the shell gets them. So the shell and `kill` stay usable while a program runs, and only typing that a program is actively waiting for is captured. `readkey` polls with a 1-tick sleep, so a waiting program uses no CPU.

## New programs

`run cat <file>` (getargs + readfile + write), `run keys` (readkey loop, exits with the character count), `run note` (creates `made.txt` via writefile). `exec <file> args...` and `run <name> args...` pass arguments.

## Verified in QEMU

`run cat motd` prints the file from ring 3; `run cat nothere` → "could not read", status 1; no args → usage, status 2; `run note` then shell `cat made.txt` shows the file the program created; `run keys` + typing `hello` echoed through the syscall and exited with status 5.

## Limits

Programs get one shared argument string, no file handles (whole-file read/write), no stdin redirection.
