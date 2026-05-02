Intentional schema-error fixture. .grex/pack.yaml has malformed YAML / wrong schema.

DO NOT MODIFY. Used by real-smoke harness to exercise manifest-error paths
(missing required `name`, non-semver `version`, `children` typed as a string
instead of a list).

Also carries a .gitignore + ignored marker file for B5 drift-vs-gitignore
regression test: `pseudo-ignored-dir/marker.txt` should NOT be flagged as
drift by `grex doctor` because the directory matches the .gitignore pattern.
