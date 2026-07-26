# ORCA gotchas (accumulating)

Living page. Add every trap encountered, newest at top, format:
**symptom → cause → fix**.

---

- **`orca --version` "fails" / does something weird** → ORCA has **no CLI flags**; it treats
  its first argument as the name of an input file, so `orca --version` tries to open a file
  literally called `--version`. There is no version subcommand → read the version from the
  **banner printed at the top of every run's output** (e.g. `Program Version 6.1.0`), or from
  the release/install directory. Implication for OrcaStudio: detect the ORCA version by parsing
  the output banner, never by shelling out `orca --version`.
- **%pal ignored, runs on 1 core** → ORCA invoked via bare name from PATH → always invoke
  with full absolute path (see orca-basics.md).
- **MPI errors at startup on a machine where serial runs work** → OpenMPI version mismatch
  with the ORCA build → install the exact version the release notes specify.
- **Huge outputs freeze naive viewers** → never read output files whole; tail/stream only.
- *(add as encountered during Phase 0+)*
