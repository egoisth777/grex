# feat-grex — index

Active feature directory for the `grex` Rust CLI + pack-protocol design.

```
openspec/feat-grex/
├── spec.md        # feature spec
└── README.md      # this index
```

## Entry points

- [`spec.md`](./spec.md) — problem, goal, success criteria, acceptance.
- [`../../progress.md`](../../progress.md) — session-resume snapshot.
- [`../../milestone.md`](../../milestone.md) — M1-M8 phased delivery + v2 backlog.
- [`../../.omne/cfg/README.md`](../../.omne/cfg/README.md) — design-doc index (15 topic docs).
- [`../../.omne/cfg/pack-spec.md`](../../.omne/cfg/pack-spec.md) — `.grex/` dir + `pack.yaml` schema.

## Status

Branch: `feat/omne-rm-design`. Design phase complete; ready to scaffold the crate (M1).

## Related

- Tool identity: `grex` (binary + crate).
- Tagline: cross-platform dev-environment orchestrator. Pack-based, agent-native, Rust-fast.
- Reference pack fixture: `grex-inst` (first-party example consumer of the pack protocol).
