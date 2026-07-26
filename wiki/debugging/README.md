# Debugging log

One page per non-trivial solved bug (>~30 min of work). Written immediately after the fix,
while context is fresh. This folder becomes the most valuable one in six months.

## Page template

```markdown
# <short-slug>.md — Symptom in one line

**Date:** · **Area:** frontend | rust-core | sidecar | orca | ssh
**Symptom:** what was observed
**Root cause:** the actual mechanism
**Fix:** what changed (commit ref)
**Lesson / rule:** if it produced a durable rule, add it to gotchas.md or CLAUDE.md too
```

Naming: `NNN-short-slug.md`, sequential.
