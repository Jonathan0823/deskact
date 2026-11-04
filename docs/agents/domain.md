# Domain Docs

This repository uses a single-context domain-doc layout.

## Before exploring

- Read `GLOSSARY.md` at the repository root, if present.
- If a root `GLOSSARY-MAP.md` exists, use it to locate and read the relevant context glossary files instead.
- Read relevant ADRs under `docs/adr/` before work in the affected area.
- If these files do not exist, proceed silently; do not flag their absence or create them upfront. The domain-modeling skill creates them lazily when terms or decisions are resolved.

## Use the glossary's vocabulary

Use terms as defined in the glossary in issues, proposals, hypotheses, and tests. If a needed concept is absent, reconsider whether the term is being introduced unnecessarily or note a genuine vocabulary gap for domain modeling.

## Flag ADR conflicts

If a proposed decision contradicts an existing ADR, surface that conflict explicitly rather than silently overriding it.
