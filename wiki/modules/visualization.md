# Module: Visualization

**Status:** not started

## Structures & trajectories
3Dmol.js viewer component; multiframe xyz → trajectory playback with frame slider.

## Orbitals / densities
`orca_plot` in batch (non-interactive) mode generates `.cube` from `.gbw`
→ 3Dmol.js volumetric isosurface (positive/negative lobes, adjustable isovalue).
Default grid 80–100; cubes cached in job dir; generated lazily on MO selection.

## Spectra
- IR: Lorentzian broadening over (freq, intensity) list; recharts; peak click →
  animate corresponding normal mode (displacement vectors from output).
- UV-Vis (Phase 6): Gaussian broadening over TD-DFT (energy, fosc).

## Watchpoints
- WebKitGTK WebGL performance — validate with a ~100-atom molecule + cube in Phase 2/3.
- Cube file parsing in JS: stream-parse, don't JSON-roundtrip through the sidecar.
