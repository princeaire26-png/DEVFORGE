# MILESTONE 05 — AI REPAIR ENGINE

## Implemented

- Safe structured repair patches restricted to the DEVFORGE project workspace.
- Patch limits: maximum 8 patches per repair, bounded patch size, and one-match replacement.
- Native repair log at `.devforge/repair-log.json`.
- Repair loop with a maximum of 3 attempts.
- Build → diagnose → patch → build → test loop for the generated native workspace.
- Controlled demo-failure injector so the repair workflow can be exercised without pretending that a failure is real.
- Build Center now exposes **Simulate failure** and **Repair build** controls.
- Repair results are displayed in the build state and recorded locally.

## Important boundary

The patch application and repair loop are real native functionality. The current automatic diagnosis contains a deliberately narrow, safe rule for the generated DEVFORGE template; it is **not yet a general-purpose AI coder**. The next AI milestone will connect provider-generated structured patches to this same safety boundary.

## Next

MILESTONE 06 — AI CODER + PATCH PROVIDER: real model-generated plans/patches, file-aware context, approval policy, and iterative validation.
