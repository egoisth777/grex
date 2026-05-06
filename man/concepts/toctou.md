# toctou

The `BoundedDir` primitive — how grex closes the path-swap TOCTOU window between `canonicalize(dest)` and the actual filesystem write. Hybrid `cap-std` (uniform) plus Linux `openat2(RESOLVE_BENEATH)` (internal acceleration).

> Canonical source: forthcoming `.omne/toctou.md` (SSOT, separate `grex-inst` repo). For now this page derives from `.omne/walker.md` §Symlink hardening, `.omne/rust-design-decisions.md` §6, `.omne/proof/impl-axiom-bridge.md` §3 (`sync_local_writes`), and `crates/grex-core/src/fs/boundary.rs` (the implementing module).

## What is TOCTOU?

> TOCTOU = Time-Of-Check / Time-Of-Use — a race-condition class where a program checks a property of a path (e.g. "this resolves to `<meta>/code/.git`") and then operates on the same path later, between which an attacker swaps the path's target.

The classic walker race window without `BoundedDir`:

```text
parent.canonicalize() → resolve(child) → fs::create_dir_all(dest) → clone(dest)
                      ▲                ▲                          ▲
                      └─ race window ──┴─ swap dest for symlink ──┘
```

An attacker who can write inside the workspace mid-flight could redirect the `clone` write to an arbitrary location — for example, replace `<meta>/code/` with a symlink to `$HOME/.ssh/`, then watch grex happily clone-over the user's keys.

Path-string-based filesystem APIs (`std::fs::create_dir_all(path)`, `std::fs::write(path, ...)`) re-walk the path on every call. Each walk is a fresh check; nothing in the standard library ties consecutive calls to the SAME inode the prior call resolved.

## Why TOCTOU matters for `grex sync`

The walker mutates the filesystem in three places — each is a TOCTOU surface if not bound to a kernel-vouched handle:

1. **Phase 1, branch 1 (clone).** `git_clone(child.url, dest, child.ref)` writes to `dest`. A path-swap attack between the validator's canonicalisation pass and the clone could redirect the write outside `<meta>`'s subtree.
2. **Phase 1, branch 2 (recover empty dest).** Same exposure as branch 1 — the recovery clone re-walks `dest`'s path.
3. **Phase 2 (`rm -rf`).** Deleting the dest after a default-deny prune-safety pass. A swap between the safety check and the unlink could redirect `rm -rf` to a location outside `<meta>` (catastrophic — see [force-prune §Blast radius](./force-prune.md#blast-radius) for the boundary contract that depends on this NOT happening).

See the 5-way classifier in [walker §Phase 1](./walker.md#phase-1--sync-direct-children-parallel) for context — branches 1, 2, and 5 are the only branches that mutate `dest`; branch 5 (`git fetch`) operates against the already-bound dest and is not a fresh path-walk.

## `BoundedDir` — the primitive

`BoundedDir` is a thin wrapper around a kernel-confirmed directory handle (a "dirfd") obtained for a path **provably contained** beneath a parent directory. Once constructed, the handle is bound to the inode the kernel resolved at construction time — a subsequent attacker swap of the parent path for a symlink cannot redirect operations performed through the handle.

```text
BoundedDir::open(parent, child_relative)? → handle bound to inode
```

Downstream operations either go through the dirfd (write confined) or compare against `BoundedDir::path` (the canonicalised, post-resolve path) — both of which the kernel has already vouched for.

The module lives at `crates/grex-core/src/fs/boundary.rs`. Visibility is `pub(crate)` — cap-std types do not leak into the public API surface, so a future implementation swap (e.g. raw `openat2` plumbing) does not bump the `grex-core` SemVer.

## Hybrid strategy: cap-std uniform, openat2 internal

Per design decision §6 in `.omne/rust-design-decisions.md`:

| Platform        | What `BoundedDir` actually does                                                              |
|-----------------|----------------------------------------------------------------------------------------------|
| Linux ≥ 5.6     | `cap-std` internally uses `openat2(RESOLVE_BENEATH)` — single syscall, kernel-enforced bound |
| Linux < 5.6     | `cap-std` falls back to `O_NOFOLLOW`-by-component verification                                |
| macOS / Windows | `cap-std` uses platform-equivalent capability handles                                         |

The `BoundedDir` API is uniform across all platforms — callers do not branch. The `openat2` acceleration on modern Linux is an internal detail.

### Why uniform cap-std rather than per-OS hand-rolling

Trade: the marginal performance of a single-syscall `openat2` versus three concrete benefits:

1. **No `unsafe` in `grex-core`.** The crate carries `#![deny(unsafe_code)]`. Hand-rolled `openat2` plumbing requires `unsafe` for the libc syscall surface.
2. **One code path to test across OSes.** Per-OS branches multiply the test matrix and the audit surface.
3. **No kernel-version branching at runtime.** cap-std handles the Linux ≥ 5.6 / < 5.6 split internally; grex-core never observes it.

## Dirfd-binding model

What "bound to an inode" actually means:

1. `BoundedDir::open(parent, child_relative)` opens `parent` as a `cap_std::fs::Dir`. The kernel resolves `parent` to an inode and gives back a file descriptor referring to that specific inode.
2. The constructor then resolves `child_relative` *through* that dirfd — the kernel walks segment-by-segment, verifying each step does not escape the parent capability (no `..`, no symlink-to-outside, no absolute redirect).
3. The returned `BoundedDir` carries the verified child dirfd. All future operations route through this fd, not through the original path string.
4. If an attacker swaps `parent/child_relative` for a symlink AFTER step 2, subsequent reads/writes through the `BoundedDir` still hit the original inode the kernel resolved at step 2 — the attacker's swap is invisible to the bound handle.

Dropping the `BoundedDir` releases the dirfd; the inode is then eligible for unlinking by other processes (this is fine — by then the walker is done with that dest).

## Path-swap attack closure

Concretely, the attack the walker now defeats:

```text
T+0   user runs: grex sync .
T+1   walker validates: canonicalize(<meta>/code/) → /home/u/proj/code (real dir)
T+2   walker calls: BoundedDir::open(<meta>, "code") → fd 17 bound to inode 99
T+3   ATTACKER (running concurrently): rm <meta>/code; ln -s $HOME/.ssh <meta>/code
T+4   walker calls: git_clone(url, BoundedDir::path(&fd17)) → writes to inode 99
                    (the original /home/u/proj/code, NOT $HOME/.ssh)
```

Without `BoundedDir`, step T+4's `git_clone` would re-walk the path string `<meta>/code/` and follow the attacker's symlink to `$HOME/.ssh`. With `BoundedDir`, the write is bound to the kernel-vouched inode from T+2.

The walker also rejects symlinks at canonicalisation gate (see [walker §Symlink hardening](./walker.md#symlink-hardening)) — but that gate runs at validation time, BEFORE the bind. `BoundedDir` is what closes the window between validation and write.

## Lean4 axiom: `sync_local_writes`

The Lean4 mechanisation models this as a bridge axiom in `proof/Grex/Bridge.lean`:

```lean
axiom sync_local_writes
    (parent : Path) (w : World) (q : Path) :
    ¬ descends q parent →
    (sync parent w).tracked q = w.tracked q ∧
    (sync parent w).lock q    = w.lock q    ∧
    (sync parent w).hasGit q  = w.hasGit q
```

> "**Bridge 3.** `sync` writes only inside its argument subtree. Every other path's `tracked`, `lock`, and `hasGit` are unchanged. This is the model-level statement of v1.2.0's parent-relative discipline."

The Rust contract that discharges this axiom is precisely the `BoundedDir` capability handle. Without it, a malicious symlink inside the subtree could cause `sync` to clobber `w.hasGit q` for a `q` that does not descend from `parent`, falsifying the axiom.

The `validate_children_paths` gate (rejects `..` and absolute segments) is **necessary but NOT sufficient** on its own; the capability-handle invariant is what closes the symlink-traversal escape window. Any change to the Rust impl that swaps `cap-std` for raw `std::fs` MUST re-prove this axiom (or bridge it via an explicit "no-symlink-escape" lemma) — `.omne/proof/impl-axiom-bridge.md` §3 documents this re-review trigger.

## What `BoundedDir` does NOT cover

- **`O_NOFOLLOW` does not protect against TOCTOU on the `parent` itself.** `BoundedDir::open(parent, child)` opens `parent` as the capability root — if the attacker swaps the parent's path to a different inode BEFORE `BoundedDir::open` is called, the bind happens against the wrong inode. The walker mitigates this by binding the cwd-meta's parent dirfd at sync-startup, before any per-child resolution fires; from then on every recursion's `parent` is itself a fd, not a path string.
- **Concurrent writers within the bound dirfd.** If a non-grex process holds a write descriptor under the bound dirfd, the walker's writes can race with theirs at the inode level. The per-pack `.grex-lock` (see [concurrency §Per-pack PackLock](./concurrency.md#per-pack-packlock)) closes the in-grex case; cross-tool coordination is out of scope for `BoundedDir`.
- **Filesystem-layer attacks.** A malicious filesystem (FUSE, network mount with adversarial server) can violate the kernel's inode-stability guarantee. `BoundedDir` assumes a non-adversarial filesystem layer.

## Cross-references

- Walker phases that perform the bound writes: [walker](./walker.md)
- `rm -rf` blast-radius contract that depends on bound deletes: [force-prune](./force-prune.md)
- Per-pack `.grex-lock` mutual exclusion (orthogonal to TOCTOU): [concurrency](./concurrency.md)
- Lockfile atomic rewrite (also dirfd-bound under v1.2.0+): [lockfile](./lockfile.md)
