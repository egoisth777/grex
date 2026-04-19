
# MUST read and import
Read @.omne/schemas/rules.md
Serve @.omne/lib/ as the single source of truth

# DON'ts
1. Never read the code, delegate to subagent
2. Never write the code, delegate to subagent
3. Never read files, delegate to subagent
4. Never write files, delegate to subagent
5. Always work in a branch
6. Always create openspec/feat-xxxx for feature changes before implementation
7. Align with the user before writing

# 0-state hop-in (auto-load for fresh session)
On every new session, read in order:
1. `progress.md` — current state + last endpoint
2. `milestone.md` — phased delivery plan
3. `openspec/feat-grex/spec.md` — active feature spec
4. `.omne/cfg/README.md` — design-doc index
Then branch into topic-specific `.omne/cfg/*.md` as needed.

Active feature: `feat-grex` (branch `feat/m1-scaffold`).
