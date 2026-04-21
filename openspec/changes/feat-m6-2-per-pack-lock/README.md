# feat-m6-2-per-pack-lock

**Status**: draft

Per-pack `<pack_path>/.grex-lock` via `fd-lock`, acquired inside pack-type plugin methods (`install`/`update`/`sync`/`teardown`). Pins the full 5-tier lock ordering: workspace-sync → semaphore slot → per-pack lock → backend lock → manifest RW lock. Prevents double-execution on the same pack across tasks and processes; guarantees deadlock-free ordering. Adds `.grex-lock` to the default managed-gitignore block.
