# Knowledge base: Taiwan official sites

Use this before web search when the question is a Taiwan government service or a daily-life official site: where to file, which office, the official URL, what to bring.

Do not answer those from memory. Do not invent a URL, a fee, or a step.

## Order

1. `kb search`, then `kb get` if you need aliases, service links, `verify_status`, or the source.
2. `kb open` (or `fetch` of a URL that just appeared in kb output), then `read`.
3. `note add` with that URL or `doc:N`.
4. `search` only if the knowledge base has no entry, the official page is blocked and you still need the text, or the question is outside Taiwan daily life. Add `--site gov.tw` when you can.

```bash
agentbox kb search "身分證 補發" --kind service --limit 5
agentbox kb get org:ris
agentbox kb open org:etax
agentbox read doc:1 --grep 補發
agentbox search "身分證 補發" --site gov.tw --max-results 5
```

## What the fields mean

- `score` is 0 to 1. Higher is better. 1.0 is an exact name or alias.
- `match` is `exact`, `partial` (the words appear inside a longer name), or `fuzzy` (typo or split words such as 報稅 inside 綜合所得稅結算申報).
- `via` is `trigram` (FTS5 substring, queries of 3+ characters) or `scan` (short query or no trigram hit).
- `verify_status`: `verified` (title checked), `connected` (HTTP 200), `blocked` (domain exists but the body was not read), `unverified` (taken from open data, not re-checked one by one).
- If `verify_status` is `blocked`, you may report `official_url` and you must say the page text was not fetched. Do not fill in fees or hours from memory.
- `kb open` returns the same shape as `fetch` (`doc`, `title`, `outline`). `from_cache: true` means a snapshot under 30 days old was reused.
- Service records from data.gov.tw dataset 146973 keep the OGDL-1.0 attribution in `$AGENTBOX_HOME/kb/ATTRIBUTION.txt`. Curated rows are official URLs only.

## When nothing matches

`error.code` is `not_found`. The hint names closer records when it has any, then a rephrase command, then `agentbox search "身分證 補發" --site gov.tw` as the last resort. Say the knowledge base has no entry. Do not guess a URL.

`kb_empty` means the database was never imported. Ask the human to run `agentbox kb import --curated`. Do not run `kb import` yourself.

## Categories

`portal`, `household`, `nhi`, `tax`, `motor`, `labor`, `welfare`, `transport`, `medical`, `utilities`, `post`, `weather`, `consumer`, `finance`, `food`, `shopping`, `housing`, `education`, `entertainment`, `payment`.

Everyday examples: `電影` and `看電影` for cinemas, `公車` and `坐公車` for city buses, `天氣` for the weather agency, `台鐵` or `高鐵` for rail, `租屋` or `租房子` for rentals, `外送` or `叫外送` for delivery. `verify_status: unverified` means the official URL is stored but this check could not read the page.

```bash
agentbox kb search 健保 --category nhi
agentbox kb search 北區國稅局 --kind org
```

臺北國稅局 (`org:ntbt`) and 北區國稅局 (`org:ntbna`) are different offices. 高速公路1968 is `https://1968.freeway.gov.tw`.
