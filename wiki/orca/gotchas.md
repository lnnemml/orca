# ORCA gotchas (accumulating)

Living page. Add every trap encountered, newest at top, format:
**symptom → cause → fix**.

---

- **%pal ignored, runs on 1 core** → ORCA invoked via bare name from PATH → always invoke
  with full absolute path (see orca-basics.md).
- **MPI errors at startup on a machine where serial runs work** → OpenMPI version mismatch
  with the ORCA build → install the exact version the release notes specify.
- **Huge outputs freeze naive viewers** → never read output files whole; tail/stream only.
- *(add as encountered during Phase 0+)*
