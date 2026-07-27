# Superseded NEAR Intents sibling design

The 2026-07-24 sibling-service decision was superseded on 2026-07-27 after
NEAR Intents review clarified the method boundary. The living design is
[NEAR Intents Verifier method — in-tree design sketch](near-intents-verifier-design.md).

This compatibility file remains so historical links do not break. An operator
may isolate the mainnet-only method in a separate process or hostname, but it
uses the same in-tree provider, service binary, migrations, and tests—not a
sibling implementation.
