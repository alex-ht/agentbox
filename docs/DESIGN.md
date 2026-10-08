# 設計筆記：輸出外層與 handle 慣例

這份文件說明 agentbox 所有子命令共用的約定。新增子命令時請遵守，讓模型只要學一次就能用全部工具。

## 1. 輸出外層（envelope）

每次執行只輸出一個 JSON 物件（預設一行、精簡格式），欄位順序固定：

```json
{"ok":true,"data":{...},"hint":null}
{"ok":false,"error":{"code":"not_found","message":"--find text does not occur in a.txt"},"hint":"Copy the exact text (including whitespace) with `agentbox file read a.txt`."}
```

- `ok`：布林值，框架只看這個欄位判斷成敗。
- `data`：成功時的結果，結構依子命令而定，但盡量扁平，欄位名稱用 snake_case。
- `error.code`：機器可判讀的錯誤碼（如 `bad_args`、`http_error`、`doc_not_found`、`not_implemented`），程式可以據此分支。
- `error.message`：一句話說明發生什麼事，包含關鍵數值（例如找到幾次、檔案有幾行）。
- `hint`：失敗時**一定要有**，內容是「下一步可以怎麼做」，最好直接附上可複製的命令。成功時可為 `null`，或提示接下來常見的動作（例如 `fetch` 之後提示用 `read`）。

結束碼：成功 0、執行錯誤 1、參數解析錯誤 2。參數錯誤同樣用外層輸出到 stdout，不會只印 clap 的純文字錯誤；`--help` 與 `--version` 則照常輸出純文字。

`--format csv` 只在 `data` 同時有 `columns` 與 `rows`（`table show`、`table query`）時輸出純 CSV，其他命令照常輸出 JSON 外層，避免模型拿到半套格式。

`--format md` 會把同一個外層轉成 Markdown：純量欄位變成項目清單、物件陣列變成表格、多行字串原樣輸出、`diff` 包在 ```` ```diff ```` 區塊內，最後一行是 `> hint: ...`。`report check` 的結果（`data` 同時有 `pass` 與 `issues`）例外，會畫成待辦清單：PASS/FAIL 標頭、分數、統計，每個問題一行 `- [ ] **error** `rule` (line N): ...`，下面接 fix。

## 2. 文件 handle

長內容不直接塞進輸出。`fetch` 把轉好的 Markdown 存進狀態目錄，回傳：

```json
{"doc":"doc:3","title":"...","url":"...","chars":19873,"sections":12,
 "outline":[{"section":1,"heading":"Overview","chars":1520}, ...]}
```

- handle 格式為 `doc:<N>`，N 從 1 遞增；讀取時也接受大寫或單純數字。
- 章節依 Markdown 標題（`#`～`######`，忽略程式碼區塊內的 `#`）切分；第一個標題之前若有文字，算成 `(top)` 章節。
- 單一章節超過 6000 字元時，會在段落處再切成 `(part k/n)`，讓每段都能一次讀完。
- 大綱最多內嵌 60 筆，其餘以 `outline_omitted` 告知數量。

`read` 的三種模式：

| 模式 | 參數 | 回傳 |
|---|---|---|
| 讀章節 | `--section N` | 該章節內容 |
| 找關鍵字 | `--grep KEYWORD` | 不分大小寫的前後文片段（每側約 240 字元，相鄰片段合併），附所屬章節編號 |
| 從頭讀 | （都不給） | 從 `--offset` 開始的內容 |

截斷規則：超過 `--max-chars`（預設 4000）時回傳 `truncated:true` 與 `next_offset`，`hint` 直接給出接續的完整命令。

## 3. 狀態目錄

```
$AGENTBOX_HOME（預設 ~/.agentbox 或 %USERPROFILE%\.agentbox）
├── config.toml   設定（Unix 上權限 0600），例如 [tavily] api_key、[search] backend
├── templates/    使用者報告範本，<名稱>.toml
├── docs/
│   ├── 1.md      轉換後的內容
│   └── 1.json    中繼資料：url、title、content_type、fetched_at
├── tables/
│   └── 1.json    表格：id、title、source、columns、rows（儲存格是 JSON 值）、created
└── notes.jsonl   筆記，每行一個 JSON：id、text、source、tag、created
```

`search --save N` 存下的結果和 `fetch` 一樣是 `doc:N`；來源是 Tavily 全文時，`content_type` 會標成 `text/markdown; source=tavily`。

## 4. 有副作用的命令

會改動使用者檔案的命令（`file write`、`file replace`，以及 `report build --out`）預設只回傳 diff 預覽與 `applied:false`；同一個命令加上 `--apply` 才寫入。`file replace` 要求 `--find` 恰好出現一次，出現 0 次或多次都會回錯並在 `hint` 說明怎麼修正（多次時可加 `--all`）。

## 5. 金鑰與機密

- 金鑰只從環境變數（`TAVILY_API_KEY`）或 `config.toml` 讀取，不接受命令列旗標，避免出現在 agent 對話紀錄與程序清單。
- 任何輸出（資料、錯誤訊息、hint）都不得包含金鑰。`search` 在回傳前會再把金鑰字串替換成 `[redacted]`，就算上游 API 把金鑰回傳在錯誤訊息裡也一樣；`config get` 只顯示公開前綴（如 `tvly-dev-****`）、長度與 FNV 短指紋。
- `config` 不放進 `schema`，`call` 也無法呼叫它：設定金鑰是使用者的事，不該經過模型。
- 測試用的服務位址可用 `AGENTBOX_TAVILY_URL`、`AGENTBOX_DDG_URL`、`AGENTBOX_BING_URL` 覆寫，單元測試用內建的 std `TcpListener` 假伺服器，不需要真的金鑰。

## 6. 工具 schema

`src/schema.rs` 有一張手寫的規格表，`agentbox schema` 由它產生 OpenAI function tool 格式；`agentbox call` 也用同一張表把 JSON 參數轉回命令列。單元測試會逐一比對規格表與 clap 定義（參數名稱、是否必填、預設值、可選值），兩邊不一致就會失敗，避免 schema 與實際行為脫節。巢狀子命令的工具名稱用底線連接，例如 `file_replace`、`note_add`。可重複的旗標（如 `search --site`）在 schema 裡是字串陣列，`call` 會展開成多個 `--site=...`。

報告工具拆成 `report_build`、`report_check`、`report_templates`、`report_template_show` 四個獨立的 function tool，而不是一個 `report` 工具加 `action` 參數。理由是小模型最容易犯的錯是參數搭配錯誤（例如對 `check` 傳了 `title`、對 `build` 傳了 `file`）；拆開後每個工具的 schema 只列自己用得到的參數，必填欄位也很明確，描述可以直接寫「下一步做什麼」。代價只是工具清單多三個項目。schema 測試會遞迴走訪任意深度的巢狀子命令（`report template show` → `report_template_show`），確保規格表與 clap 定義同步。

## 7. 報告範本與檢查

### 範本格式

範本是 TOML，內建的 `brief`、`compare`、`top-n`、`exec-lookup` 以 `include_str!` 編進執行檔（原始檔在 `templates/`）。載入順序：`--template` 看起來像路徑（含 `/`、`\` 或 `.toml`）就讀檔；否則先找 `$AGENTBOX_HOME/templates/<名稱>.toml`，再找內建範本。解析時 `deny_unknown_fields`，拼錯欄位會回 `bad_template` 而不是被忽略。

頂層欄位：`name`、`description`、`title_level`（0 表示不要求標題）、`min_words` / `max_words`（全文，不含標題與 Sources）、`citation_style`（`inline-link` | `footnote` | `numbered`）、`require_sources_section`、`sources_heading`、`sources_level`、`min_sources`、`sources_from_notes`、`forbid`。段落 `[[section]]` 的 `heading` 若含 `{n}` 就是編號段落，`{title}` 代表項目名稱，數量由 `repeat_min` / `repeat_max` 決定，可用 `--n` 一次覆寫兩者；`table = true` 的段落可用 `--columns` 覆寫欄位。`--n` 或 `--columns` 用在不適用的範本上會回 `bad_args`，不會默默無效。

### build 的慣例

- TODO 一律是 `<!-- TODO(k): 說明 -->`，k 在整份文件中唯一，`file replace --find` 永遠只會命中一處。說明文字來自段落的 `hint`，並附上字數要求與引用格式範例。
- 筆記分配：tag 是 `item2`、`item-2` 或 `2` 時放進第 2 個編號項目；否則 tag 符合段落 `keywords` 或標題中長度 ≥ 4 的字時放進該段。分不到的筆記放進 Sources 前的 `<!-- NOTES ... -->` 註解區塊，提醒模型搬移後刪除（`check` 會對殘留的區塊發警告）。
- 引用依範本的 `citation_style` 產生，Sources 段落列出筆記來源（有 fetch 過的文件會用它的標題）；來源不足 `min_sources` 時加一個 TODO，並列出已 fetch 但還沒用到的文件當候選。
- 輸出含 `outline`（行號、層級、標題）與 `todos` 數量，模型不用重讀整份檔案就知道結構。

### check 的慣例

- 用逐行解析的 Markdown 解析器（不依賴 pulldown-cmark），以便精準回報行號；fenced 與縮排程式碼區塊內的內容一律略過，HTML 註解不計字數。
- 每個 issue 是 `{severity, rule, line, message, fix}`。`fix` 必須具體：能用單行取代修好的（標題層級、編號格式、Sources 名稱、多餘 H1、近似段落名稱）直接給 `agentbox file replace "檔名" --find "原文" --replace "新文" --apply`；其餘給出要加什麼、加在哪一行之後、還差多少字。
- 編號段落先找完全符合格式的標題，再找近似寫法（`1)`、`1:`、`1-`、`1、`，或層級不對）；若數量還不夠，緊接在編號區後、且不是其他範本段落的標題（如 `## Rocket`）會被視為漏了編號的項目，直接給改名命令。
- 分數 = 100 − 12 × 錯誤數 − 4 × 警告數（最低 0），`pass` 等於沒有錯誤。檢查沒過不算執行失敗：外層仍是 `ok:true`，結束碼 0。
- `sources_from_notes = true` 時，報告裡出現的每個網址都必須能在 notes 或 docs 中找到（比對前先正規化網址），否則回報 `source_not_in_notes` 並建議先 `fetch` 再 `note add`。

## 8. 抽取（extract）

- **固定規則，不用語言模型**：所有 kind 都是正規表示式加上少量啟發式，同樣的輸入一定得到同樣的輸出。模型只要選 `--kind`，不需要自己寫正規表示式。
- **輸入**：一或多個 `doc:N`／檔案（位置參數，或 `--from doc:1,doc:2`，逗號分隔也可以，重複的會去掉）。.html／.htm 檔會先用和 `fetch` 相同的轉換器轉成 Markdown。
- **前處理**：逐行處理並略過程式碼區塊；比對前先把 Markdown 還原成純文字（去掉標題與清單符號、`**`、反斜線跳脫、表格外框，連結只留文字），並移除 `[1]`、`[citation needed]` 這類註腳標記。`--section` 用和 `read` 相同的章節編號，`--grep` 是不分大小寫的子字串，先過濾行再抽取。
- **項目欄位**：每個項目都有 kind 專屬欄位，加上 `context`（約 120 字元，必要時加 `…`）、`section`、`line`、`doc`、`url`。`links` 的 `url` 是連結本身，文件網址放在 `source`。
- **去重**：同一份文件內，以 kind 的正規化值去重（價格：數值＋幣別＋期間＋單位；日期：ISO 值；人名：姓名＋職稱；連結：網址）。不同文件的相同項目會分別保留，這樣才看得出各來源的說法。
- **限制輸出**：`--limit` 預設 20，`total` 永遠是去重後的總數；截斷時 `truncated:true`，hint 建議 `--grep`／`--section`／`--limit`。`--save-table` 存的是全部項目。
- **價格**：前綴符號／代碼（`US$`、`NT$`、`€`、`USD`…）或後綴（`元`、`円`、`EUR`、`dollars`），可帶量級（`k`、`million`、`萬`），之後讀「期間／單位」尾巴（`/user/mo`、`per seat per year`、`billed annually`、`每月`、`/GB`），期間與單位各只取第一次出現的值，所以「`/month`, billed annually」仍是 month。價格區間（`$12–19`）取下限，`raw` 保留原文。`$` 依文件網域的國碼推斷幣別（`.tw` → TWD、`.ca` → CAD…，否則 USD），`¥` 在 `.cn` 為 CNY，其他為 JPY，hint 會提醒這是推斷。
- **日期**：民國年、`年月日`、ISO（可帶時間）、英文月名（月日年、日月年）、數字（預設美式月/日，第一段 >12 才當日/月，並標 `ambiguous`）、月份精度（`Jan 2024`）。先比對較長、較明確的格式，已比對的範圍會遮罩，最後用 chrono 驗證日期確實存在。
- **人名**：句型比對（「NAME, ROLE of ORG」、「ROLE NAME」、「NAME (ROLE)」、「ROLE: NAME」、中文職稱＋2–3 個漢字），每個句型有固定 `confidence`（0.5–0.9），加上組織字、職稱字的排除清單。輸出一律附 hint，說明這是啟發式結果。
- **表格**：Markdown 管線表格與殘留的 HTML `<table>` 片段。每張表存成 `tbl:N`，標題取所在段落標題（去掉 `(part k/n)`）；內容完全相同的表格會重用既有 handle，重跑 extract 不會一直長出新表。

### fetch 的表格轉換

htmd 預設會把 `<table>` 攤平成文字。`fetch` 註冊了自己的 table handler：至少 2 欄 2 列、且每格不超過 300 字元的表格轉成 Markdown 管線表格（`<th>` 或第一列當表頭，`colspan` 補空格，巢狀表格攤平，儲存格內的連結保留為 `[文字](絕對網址)`，`|` 會跳脫）；其他當成排版用表格，照原本方式轉成文字。`rowspan` 目前不展開。

## 9. 表格（table）

- **來源**：`tbl:N`，或依副檔名／內容判斷的 CSV（RFC 4180，含 BOM 與引號）、TSV、JSON（物件陣列、二維陣列、純量陣列，或物件內深度 3 以內的第一個陣列，所以 agentbox 自己的輸出也能直接匯入）、Markdown（取第一張表，並回報檔案裡共有幾張）。空白表頭補成 `columnN`，重複表頭加 `_2`。
- **型別推斷**：非空白儲存格有 80% 以上是數字就是 `number`（有幣別時是 `currency`），80% 以上是日期就是 `date`，否則 `text`。`-`、`—`、`n/a`、`null` 視為空白。
- **數值解析**：`$1,299/yr`、`US$ 49 per user/month`、`€9,99`（單獨的「逗號＋兩位數」視為小數點）、`1.2萬`、`3.5 billion`、`12%` 都會轉成數字；數字後面的文字不能再含數字。資料量（`512 MiB`、`4 GiB`、`1 TB`）換算成 G 單位（GB 與 GiB 視為同級），所以 `Memory >= 2` 符合直覺。計算結果四捨五入到 12 位有效數字，避免浮點雜訊。
- **where**：`欄位 運算子 值`，運算子只有 `= != < <= > >= contains startswith`（也接受 `==`、`<>`、`starts with`），值可加引號。值能解析成數字就用數字比較，能解析成日期就用日期比較，否則用不分大小寫的文字比較；不支援 OR、NOT 與括號，刻意保持「一行一個條件」。
- **sort**：`-欄位` 遞減，`欄位` 或 `+欄位` 遞增，也接受 `欄位 desc`；多個鍵穩定排序。欄位有一半以上可轉成數字時以數字排序，否則以日期、再以文字排序；空白與無法比較的儲存格一律排最後（不論升降冪）。
- **group／agg**：`--group-by 欄位 --agg "count,sum:Price,avg:Score,min:X,max:X"`，輸出欄位為 `count`、`sum_Price`…；順序固定是 where → group → sort → select。
- **欄位比對**：完全相同 → 不分大小寫 → 只比英數字（`price usd` 對到 `Price (USD)`）；都失敗時回 `unknown_column`，列出全部欄位、以編輯距離找最接近的名稱，hint 給出改好的參數。hint 裡的命令會依內容加引號（含 `$` 的欄位用單引號，避免 shell 展開）。
- **儲存**：`table import` 與 `query --save` 都會寫 `tables/N.json`；與最近 200 張表中內容完全相同的表會重用 handle。`query` 顯示預設 20 列，只有明確給 `--limit` 時存下的表才會截斷。
- **匯出**：`table export --out` 依副檔名輸出 CSV／TSV／Markdown／JSON（物件陣列），走 `file write` 相同的預覽／`--apply` 流程。
- **schema**：`table_show`、`table_query`、`table_import`、`table_export` 是四個獨立工具（理由同第 6 節）；`extract` 是單一工具加上 `kind` 列舉，因為所有 kind 共用同一組參數，拆開只會讓工具清單變長。
