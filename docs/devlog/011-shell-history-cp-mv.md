# Devlog 011 — A richer shell: history, cp, mv

**Milestone:** the shell gained command **history** and filesystem **`cp`** / **`mv`**. Small additions, but they make it feel like a real terminal — and two of them cost almost no new code, which is the interesting part.

## Command history

Every shell remembers what you've typed. Ours now does too: when you press Enter on a non-empty line, the command is pushed onto a `Vec<String>` that lives on the heap, capped at the last 32 so it can't grow forever. The `history` command just walks that vector and prints it numbered.

Two things worth noticing:

- It's heap-backed, which is only possible because of Stage 5. A growable list of arbitrary-length strings is exactly the kind of thing you *can't* do without dynamic allocation.
- The static is declared with `Mutex::new(Vec::new())` — no lazy initialization needed, because `Vec::new()` is a `const fn` (an empty vector allocates nothing). It only touches the heap once you actually push a command.

What this version *doesn't* do yet is up-arrow recall — pressing Up to bring back a previous command. That needs handling the arrow keys' raw scancodes and redrawing the input line on screen, which is a fiddlier job; listing history is the useful 80% for far less complexity.

## cp and mv — built from what's already there

Here's the satisfying part. `cp` and `mv` needed **zero** new filesystem code. They're composed entirely from the primitives Stage 9 already exposed:

- **`cp src dst`** = `read(src)` then `write(dst, contents)`.
- **`mv src dst`** = `read(src)`, `write(dst, contents)`, then `remove(src)`.

That `mv` is literally "copy, then delete the original" is not a shortcut — it's *what move is*, at this level. (Real filesystems optimize a same-directory `mv` into a rename that doesn't copy bytes, but the observable behavior is identical.)

This is the payoff of designing a small, clean interface back in Stage 9. Because the filesystem exposed `read` / `write` / `remove`, new commands fall out of combining them — no new core logic, no new bugs in the storage layer, just a few lines of shell glue. Good primitives compound.

## The shape of an OS

Step back and look at the command list now: `ls`, `cat`, `write`, `rm`, `cp`, `mv`, `mem`, `history`, `clear`. That's recognizably a shell. None of it required new low-level machinery this time — it's all assembled from the foundations (interrupts, heap, filesystem) built in earlier stages. That's a sign the foundations are solid: once they're right, *features* are cheap.

## Next

The two remaining heavyweight milestones — preemptive scheduling and a real disk driver — are deliberately being saved for slower, multi-session work, since they involve context-switching and device I/O that don't fit a single clean session.
