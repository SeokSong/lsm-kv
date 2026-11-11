# Crash Recovery

**Status:**

## Guarantee

<!-- What exactly does the store promise after a crash? -->

## Startup steps

<!-- Numbered, in order: manifest, stray files, WAL replay, ... -->

## Failure scenarios

| Scenario | What's on disk | What recovery does | Data lost? |
|---|---|---|---|
| | | | |

## Why replay is safe

<!-- Idempotency: why is replaying an already-flushed record harmless? -->

## Tests

<!-- How will you simulate crashes? Truncation, kill -9, fault injection? -->

## Open questions

## See also

[WAL.md](WAL.md) · [FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md)
