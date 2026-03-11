# Task Plan: Build `codebase_review_guide.md`

## Plan
- [x] Map full runtime startup flow from `src/main.rs` across all modules
- [x] Document each feature area with current behavior, key files, and data flow
- [x] Add feature-by-feature "how to read" guidance for safe code changes
- [x] Include practical review sequence for understanding code before fixing issues
- [x] Review for completeness against `src/`, `docs/`, `plans/`, and `tests/`

## Notes
- Goal: help a human maintainer quickly understand the current generated codebase before refactoring/fixes
- Scope: current implementation, not target architecture

## Review
- Completed deliverable: `codebase_review_guide.md`
- Coverage includes startup wiring, feature modules (types/crypto/consensus/timer/router/state/contracts/storage/p2p/rpc/metrics/config), test/devops context, and end-to-end runtime flows
- Added maintainer-oriented reading paths and risk-aware review checklist before code changes
