## Summary

<!-- What changed, and why? Keep it to a few sentences. -->

## Related issue

<!-- Use `Closes #123` to close the issue on merge, or `Refs #123` to link without closing. -->

Closes #

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Documentation
- [ ] Refactor or maintenance
- [ ] CI, tooling, or dependencies
- [ ] Breaking change

## Affected area

- [ ] `playact` engine
- [ ] `playact-macros` / `bt!`
- [ ] Examples, benchmarks, or tests
- [ ] CI or repository automation

## Compatibility

<!-- Delete this section if not applicable. Note public API changes, config schema, snapshot format, feature flags, MSRV, or no_std impact. -->

## Test plan

<!-- CI covers fmt, clippy, all-feature workspace tests, docs, examples, and the embedded no_std check. Add the focused commands or manual scenarios a reviewer should reproduce. -->

```text
cargo test -p playact --all-features
```

## Checklist

- [ ] Tests cover the new behavior or regression.
- [ ] User-facing docs (README or rustdoc) are updated when behavior or API changes.
- [ ] Compatibility impact is documented above.
- [ ] No secrets or personal data are included in code, logs, or screenshots.
