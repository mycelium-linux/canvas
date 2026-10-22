# X war-room feed

`canvas-x-trends` is the only component that talks to X. It calls the official
recent-search API with application-only Bearer authentication, normalizes and
deduplicates posts, applies local include/exclude filters, ranks by recency and
weighted public engagement, then emits the shared `canvas.social.trends.v1`
contract to Canvas.

Bluesky uses the same normalized contract through `canvas-bsky-trends`. Search
is served by the public Bluesky AppView (`api.bsky.app`); account identity and
owned records remain authoritative on the account's PDS. No Bluesky credential
is needed for this read-only war-room feed.

Copy `config/x-trends.example.json` outside the repository and edit its saved
feeds. Put the credential in the service environment, never in that file:

```sh
export X_BEARER_TOKEN='...'
cargo run -p canvas-feeds --bin canvas-x-trends -- --config /etc/canvas/x-trends.json
```

Each saved feed combines its raw `query`, topic phrases, `from:` account terms,
and optional language. Include/exclude filters are case-insensitive local
filters. Duplicate post IDs merge their matching saved-feed IDs. A partial API
failure is emitted as `degraded`; a total failure retains the last snapshot and
also marks it degraded, so the wall never mistakes stale data for healthy data.

The manual recovery runbook is: confirm `X_BEARER_TOKEN` is present without
printing it, run once with `--once`, inspect the producer error/status, then
restart the long-running process. HTTP status failures are intentionally loud.

For systemd, install `config/linux/canvas-x-trends.service` as a user unit,
install the binary and config at the paths named there, and create
`/etc/canvas/x-trends.env` containing `X_BEARER_TOKEN=...` with mode `0600`.
The token is read only by the producer and is never serialized onto Canvas.
