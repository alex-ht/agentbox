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

- `score` is 0 to 1. Higher is better. 1.0 is an exact name or alias. Several words are OR-matched: a hit can cover only some of them, and more matched words rank higher. Full coverage ranks first.
- `matched_terms` lists the query words that actually hit that row. Generic words (官網, 官方網站, 網站, 網址, 首頁, 怎麼, 哪裡, 申請, and close variants) are low-weight and do not have to appear.
- `match` is `exact`, `partial` (the words appear inside a longer name), or `fuzzy` (typo or split words such as 報稅 inside 綜合所得稅結算申報).
- `via` is `trigram` (FTS5 substring, when every content word is 3+ characters and trigram found rows) or `scan` (a short word, or trigram found nothing).
- `which` lists the legal `kb open --which` values for that hit. `official` is always included. Others are that record's service links (`apply`, `form`, `page`, `email`).
- `verify_status`: `verified` (title checked), `connected` (HTTP 200), `blocked` (domain exists but the body was not read), `unverified` (taken from open data, not re-checked one by one).
- If `verify_status` is `blocked`, you may report `official_url` and you must say the page text was not fetched. Do not fill in fees or hours from memory.
- `kb open` returns the same shape as `fetch` (`doc`, `title`, `outline`). `from_cache: true` means a snapshot under 30 days old was reused.
- Service records from data.gov.tw dataset 146973 keep the OGDL-1.0 attribution in `$AGENTBOX_HOME/kb/ATTRIBUTION.txt`. Curated rows are official URLs only.

## When nothing matches

`error.code` is `not_found`. The hint is about this query: drop generic or extra words, search one term at a time, or try the formal name it names. It does not suggest an unrelated example. Web search is last, as `agentbox search "身分證 補發" --site gov.tw`. Say the knowledge base has no entry. Do not guess a URL.

If you passed `--category` and the hint says `drop --category` plus `hits in other categories`, rerun without `--category`. The category filter hid the hits.

`kb_empty` means the database was never imported. Ask the human to run `agentbox kb import --curated`. Do not run `kb import` yourself.

## Categories

`portal`, `household`, `nhi`, `tax`, `motor`, `labor`, `welfare`, `transport`, `medical`, `utilities`, `post`, `weather`, `consumer`, `finance`, `food`, `shopping`, `housing`, `education`, `entertainment`, `payment`, `general`, `media`, `social`, `search`, `tech`.

`kb import --popular` loads the bundled Chrome UX Report seeds (CC BY 4.0). A popularity bucket only breaks ties; it does not outrank a better text match. Do not run `kb import` yourself.

Everyday examples: `電影` and `看電影` for cinemas, `公車` and `坐公車` for city buses, `天氣` for the weather agency, `台鐵` or `高鐵` for rail, `租屋` or `租房子` for rentals, `外送` or `叫外送` for delivery. `verify_status: unverified` means the official URL is stored but this check could not read the page.

```bash
agentbox kb search 健保 --category nhi
agentbox kb search 北區國稅局 --kind org
agentbox kb open org:etax --which official
```

`kb open --which` must be one of the `which` values from `kb search` or `kb get`. A bad value returns `bad_args` and lists the legal ones.

臺北國稅局 (`org:ntbt`) and 北區國稅局 (`org:ntbna`) are different offices. 高速公路1968 is `https://1968.freeway.gov.tw`.
