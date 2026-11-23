# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/hushink/playact/compare/playact-v0.1.2...playact-v0.2.0) - 2026-11-23

### Added

- complete migration tooling backlog
- add JSON migration plans and report schema
- add migration plan explain and rule provenance
- compose migration manifests from fragments
- report migration rule matches and add strict mode
- add manifest schema and init template
- stage migrated include graphs with --output
- add batch value maps and patch previews
- add migration value rules and field diffs
- add declarative migrate command
- add configuration version migration pipeline
- add check command for lint and formatting
- convert include graphs recursively
- format include graphs recursively
- diff shared include file contents
- detect moved nodes in config diff
- add diff command to playact-cli
- add fmt command to playact-cli
- auto-detect include formats in FsIncludeResolver
- add graph command to playact-cli
- add bundle command to playact-cli
- resolve includes when linting configurations
- add convert and watch to playact-cli
- add playact-cli workspace tools
- lint JSON, TOML, YAML, and RON configs
- add multi-file JSON reports to config lint
- add a config lint preflight example
- render a Markdown node catalog from registry metadata
- document keys and type key values in JSON schema
- add descriptions and params to config schema
- add registry schema export example
- inject registry names into config JSON schema
- serialize trace baselines as JSON
- parameterize config subtree templates
- reference code-built subtrees from config
- add save and resume example
- serialize snapshots as JSON
- show reload restore metadata in inspector
- bundle reload metadata into a report
- report rejected state restores
- show reload metadata in examples
- audit restored node state after reload
- demo configured boundaries in showcase
- configure scoped and contract subtrees
- report rename on moved tree nodes
- add end-to-end reload audit example
- expose diff history retention bounds
- add machine-readable status endpoint
- timestamp diff history entries
- watch include files during hot reload
- fetch diff history incrementally
- parse persisted tree diff json
- give diff history monotonic revisions
- inspect full diff history
- keep diff history and add diff json endpoint
- detect renamed nodes in tree diffs
- add tree diff json and inspector views
- add structural tree diffs to reloads
- report node id paths and audit ids
- reject duplicate node ids in builder
- reject duplicate node ids and test showcase
- add config-driven battle AI showcase
- support services in tree configuration
- add fallible and async action config nodes
- add utility, event, and value config nodes
- add switch and predicate-based config nodes
- add HTML inspector and debug server
- preserve node state across config reloads
- add scoped subtree blackboards
- add Switch and live tree visualization
- add BtNode derive macro
- add trace recording and replay verification
- add subtree contracts
- add probability and while decorators
- add remote response verification and prometheus metrics
- add node-id-keyed snapshots
- carry node ids through trace events and stats
- add config node ids, test driver, and remote config limits
- add events, fallible actions, and blackboard persistence
- add stable node ids and config schema validation

### Other

- add bilingual configuration reference
- list reload module in the API map
- add bilingual API usage guide
- make trace events non-exhaustive

## [0.1.2](https://github.com/hushink/playact/compare/playact-v0.1.1...playact-v0.1.2) - 2026-11-22

### Fixed

- resolve clippy warnings in utility selector and tree restore
