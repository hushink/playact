# TODO

Forward-looking backlog for `playact`. Completed work is covered by the READMEs,
`crates/playact/docs/`, and git history. User-facing items require tests,
bilingual documentation, and CI smoke coverage when applicable.

## Next

### Per-file migration rule coverage

- [ ] Track `matched` / `changed` counts per configuration file instead of only
  per root include graph.
- [ ] Add per-file counters to `playact migrate --json` without double-counting
  shared include files.
- [ ] Add an opt-in strict mode (for example `--strict-files`) that fails when an
  executed rule matches no objects in a file whose migration step ran.
- [ ] Cover diamond include graphs, mixed-version files, and shared includes in
  tests.

Why: root-level totals can hide a rule that silently fails on one included file.

### Plan schema for `--explain --json`

- [ ] Derive a JSON Schema for the machine-readable migration plan.
- [ ] Expose it through `--plan-schema`, or fold it into a unified schema export.
- [ ] Validate plans and reports against their schemas in CI.

Why: `--report-schema` exists, but the plan output is not yet schema-described.

### Unified schema export

- [ ] Consider `playact migrate --schemas DIR` to write manifest, report, and
  plan schemas together.
- [ ] Write transactionally and refuse to overwrite existing files.
- [ ] Keep individual `--manifest-schema` / `--report-schema` modes working.

Why: CI and editor setup currently needs multiple shell redirects.

## Later

### Additional declarative operations

- [ ] Evaluate `copy_field` and `move_field` where rename semantics are
  insufficient.
- [ ] Evaluate nested field-path expressions only if real configurations need
  them.
- [ ] Keep the manifest schema, editor completion, and operation docs in sync.

Why: expand the rule language only for demonstrated configuration needs.

### Machine-readable format versioning

- [ ] Decide whether the explain plan and report schemas need an explicit format
  version.
- [ ] Add one only when a backward-compatible extension is no longer possible.

### Performance and robustness

- [ ] Benchmark migration against large include graphs and high rule counts.
- [ ] Add property tests for fragment confinement, cycle detection, and stable
  merge ordering.
- [ ] Exercise multi-version chains when `CURRENT_VERSION` increases beyond `1`.
