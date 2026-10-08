# agentbox

給「小型語言模型 agent」用的瑞士刀：一個靜態連結的單一執行檔，像 busybox 一樣把日常研究與電腦工作常用的功能收在一起。Linux 與 Windows 都能跑。

小模型最常卡在「工具之間的膠水」：抓網頁後要自己寫 Python 清 HTML、要用 `grep | jq` 找數字、算個漲跌幅還得開 REPL。agentbox 把這些膠水做進工具裡，模型只要選對子命令、填對參數，就能完成像 PinchBench 那類任務：查股價、找活動、市場調查、Polymarket 簡報、查高階主管、深度研究、競品研究、找開源替代品、比價、IT 採購、歐盟法規、BYOK 最佳實務等。

> 目前版本：v0.2。

## 設計原則

- **子命令少而完整**：控制在 12–15 個，一律「動詞（-名詞）」命名，參數攤平。不需要管線、正規表示式或 jq；過濾、排序、加總都做成內建選項。
- **輸出格式固定**：預設輸出一行精簡 JSON，加上 `--format md` 可改成 Markdown；表格結果還能用 `--format csv`。所有子命令都用同一個外層結構：
  - 成功：`{"ok":true,"data":...,"hint":null}`
  - 失敗：`{"ok":false,"error":{"code":"...","message":"..."},"hint":"下一步建議"}`
  - 出錯時一定附上 `hint`，告訴模型下一步該怎麼做，而不是只丟一句錯誤訊息。
- **長內容給 handle，不直接灌進上下文**：`fetch` 只回傳 `doc:3` 這類 handle 和章節大綱，之後再用 `read` 依章節或關鍵字讀取需要的部分。文件存在狀態目錄（預設 `~/.agentbox/`，Windows 為 `%USERPROFILE%\.agentbox`，可用環境變數 `AGENTBOX_HOME` 改位置）。
- **有副作用的操作先預覽**：修改使用者檔案的命令預設只顯示 unified diff，確認後加 `--apply` 才真的寫入。（agentbox 自己狀態目錄裡的 docs、notes 屬於工作暫存，會直接寫入。）
- **說明簡短**：每個子命令的 `--help` 只有幾行，附 1–2 個範例，小模型讀得完。
- **可直接接進 agent 框架**：`agentbox schema` 輸出 OpenAI 風格的 function tool JSON schema，每個子命令都能當成獨立工具；`agentbox call <name> '<json>'` 再把工具呼叫轉回命令列執行。

## 子命令

| 子命令 | 用途 | 狀態 |
|---|---|---|
| `fetch <url>` | 抓網頁（rustls，不依賴 OpenSSL），轉成乾淨的 Markdown，存成 doc handle，回傳標題與大綱 | ✅ 可用 |
| `read <doc>` | 依 `--section N`、`--grep 關鍵字` 或 `--offset` 讀取文件，超過 `--max-chars` 會截斷並告訴你怎麼接著讀 | ✅ 可用 |
| `calc <expr>` | 四則運算、次方、`round(x,2)`、`min/max/sum/avg` 等 | ✅ 可用 |
| `now [--tz]` | 目前時間（ISO 8601）、星期、UTC 偏移，可指定時區 | ✅ 可用 |
| `file read\|write\|replace` | 讀取文字檔（可指定行範圍）；寫入與精確取代會先給 diff，`--apply` 才寫入；`--todo N` 直接取代報告骨架的第 N 個 TODO，`--content -`／`--replace -` 從標準輸入讀內容 | ✅ 可用 |
| `note add\|list\|clear` | 暫存筆記與來源清單，跨呼叫保留，方便最後引用；`clear` 在開始新任務前清掉舊筆記（同樣要 `--apply`） | ✅ 可用 |
| `report build\|check\|templates\|template show` | 依範本產生報告骨架（標題、編號、TODO、筆記與引用、Sources）；檢查報告結構並給出可直接執行的修正命令 | ✅ 可用 |
| `search <query>` | 網路搜尋：有 Tavily 金鑰就用 Tavily，否則用免金鑰的 DuckDuckGo（被擋時改用 Bing）；`--save N` 可把前幾筆直接存成 doc | ✅ 可用 |
| `config set\|get\|unset\|path` | 管理設定（例如 Tavily、Stooq API 金鑰），金鑰一律遮罩顯示 | ✅ 可用 |
| `schema` | 輸出所有子命令的 function tool schema | ✅ 可用 |
| `call <name> <json>` | 用 schema 名稱 + JSON 參數執行工具（給框架用） | ✅ 可用 |
| `skill install` | 把內建的 Agent Skill（教模型怎麼用 agentbox 的說明檔）複製到 skills 目錄 | ✅ 可用 |
| `extract <doc...> --kind K` | 從一或多份文件抽出表格、價格、日期、人名職稱、連結、數字、email，每筆附來源；`--save-table` 存成 `tbl:N` | ✅ 可用 |
| `table show\|query\|import\|export` | `tbl:N` 或 CSV／TSV／JSON／Markdown 表格的檢視、過濾、排序、分組加總與匯出，不用寫程式 | ✅ 可用 |
| `quote get\|history\|search` | 股票、指數、匯率、加密貨幣的即時報價、歷史價格與代號查詢（Yahoo，免金鑰；Stooq 備援） | ✅ 可用 |
| `market search\|get\|trending\|history` | Polymarket 預測市場：關鍵字搜尋、熱門、單一活動細節與機率走勢（唯讀，免金鑰） | ✅ 可用 |

所有子命令都已實作，`agentbox schema --implemented-only` 會列出全部 27 個工具（`config`、`skill`、`schema`、`call` 是給人用的，不算在內）。

## 搜尋與 Tavily 金鑰

`search` 支援三種後端，用 `--backend auto|tavily|ddg|bing` 指定，輸出的 `backend` 欄位會寫明實際用了哪一個：

- **tavily**：搜尋品質最好，還能用 `--news`（只找新聞）、`--deep`（較深入，耗 2 點額度）、`--answer`（附一段摘要回答）。需要自備 [Tavily](https://tavily.com) API 金鑰。
- **ddg**：DuckDuckGo HTML 版，不需金鑰。
- **bing**：免金鑰的最後備援，從伺服器 IP 抓時結果常常不太相關，請自行確認。
- **auto**（預設）：有設定金鑰就用 Tavily；沒有就用 DuckDuckGo，遇到機器人驗證頁時自動改用 Bing，並在輸出加上 `fallback_from` 與說明。用免金鑰後端時，`hint` 會提醒你設定 Tavily 金鑰可以得到更好的結果。

常用選項：`--max-results N`（預設 5，上限 20）、`--site 網域`（可重複）、`--exclude-site 網域`、`--days N` 或 `--time day|week|month|year`、`--save N`（把前 N 筆存成 `doc:` handle，最多 5 筆；Tavily 會直接用它抓好的全文，其他後端則自動 `fetch` 該頁）。

```bash
agentbox search "EU AI Act general-purpose AI obligations" --site europa.eu --save 2
agentbox search "Nvidia" --news --time week --answer
agentbox search "open source Notion alternative" --exclude-site reddit.com --max-results 8
```

輸出範例（節錄）：

```json
{"ok":true,"data":{"backend":"tavily","query":"Who is the CEO of Nvidia","answer":"According to the sources, the CEO of Nvidia is Jensen Huang. ...",
 "results":[{"rank":1,"title":"Jensen Huang - Wikipedia","url":"https://en.wikipedia.org/wiki/Jensen_Huang","snippet":"...","score":0.892,"doc":"doc:1"}, ...]},
 "hint":"Saved results can be read with `agentbox read doc:N --grep KEYWORD`."}
```

### 設定 Tavily 金鑰

金鑰的讀取順序是：環境變數 `TAVILY_API_KEY` 優先，其次是狀態目錄裡的 `config.toml`（`[tavily]` 區段的 `api_key`）。**刻意不提供命令列旗標**，因為旗標會出現在 agent 的對話紀錄和系統的程序清單裡。

Linux / macOS（寫進 `~/.bashrc` 或 `~/.zshrc` 就會一直生效）：

```bash
export TAVILY_API_KEY="tvly-你的金鑰"
```

Windows PowerShell：

```powershell
# 只對目前視窗有效
$env:TAVILY_API_KEY = "tvly-你的金鑰"
# 永久寫入使用者環境變數（新開的視窗才會生效）
[Environment]::SetEnvironmentVariable("TAVILY_API_KEY", "tvly-你的金鑰", "User")
```

或存進 agentbox 的設定檔。建議用 `-` 從標準輸入貼上，金鑰就不會留在 shell 歷史紀錄裡：

```bash
agentbox config set tavily.api_key -    # 貼上金鑰後按 Enter
agentbox config get                      # 顯示來源與遮罩後的值，例如 "tvly-dev-****"，外加一組指紋方便辨識
agentbox config path                     # 設定檔位置
agentbox config unset tavily.api_key
```

設定檔在 Unix 上會以 0600 權限寫入。agentbox 的任何輸出、錯誤訊息和 hint 都不會印出金鑰本身；`config get` 只顯示前綴、長度和一組無法還原的短指紋。

> ⚠️ **千萬不要把金鑰 commit 進版本庫。** 專案附了 `.env.example` 當範本；如果你用 direnv 或 dotenv 之類的工具，把它複製成 `.env` 再填值，`.env` 已列在 `.gitignore`。agentbox 本身不會讀 `.env`，只看環境變數和狀態目錄的設定檔。`config` 子命令也刻意不放進 `schema`，避免 agent 經手金鑰。

## 報告：`report build` 與 `report check`

很多評分器對報告格式非常挑剔：題目要求 `## 1.`，你寫成 `### 1.` 就算錯；少一個項目、少了 Sources 段落、某段沒附來源，也都會扣分。小模型最常在這種地方翻車，所以 agentbox 把「格式」交給範本處理：

- `report build`：依範本產生骨架。標題層級和編號一字不差，需要模型填的地方都是 `<!-- TODO(k): ... -->`（k 不重複，用 `file replace report.md --todo k --replace "內容" --apply` 就能整段取代，不必複製整個註解），筆記會依 tag 放進對應段落並附上引用，最後列出 Sources。預設只預覽，加 `--apply` 才寫檔。
- `report check`：唯讀，不改檔。回傳 `{pass, score, issues, stats}`，每個問題都有行號和**具體**的修正方式；標題層級、編號這類問題直接給一行可執行的 `agentbox file replace ... --apply`。
- `report templates` 列出所有範本，`report template show <名稱>` 印出範本的 TOML。

### 內建範本

| 範本 | 結構 | 字數 | 最少來源 |
|---|---|---|---|
| `brief` | Summary、Key Findings（需引用） | 120–900 | 2 |
| `compare` | Summary、Comparison（表格，欄位預設 Option / Price / Strengths / Weaknesses，可用 `--columns` 改）、Recommendation | 150–1200 | 3 |
| `top-n` | `## 1. 標題` … `## N. 標題`，每項都要引用（`--n` 設定項目數，預設 3） | 100–1500 | 3 |
| `exec-lookup` | Summary、Role and Background、Key Facts（後兩段需引用） | 80–800 | 2 |
| `market-brief` | Overview、`## 1. 市場` … `## N. 市場`，每項都要引用、寫出機率 `%` 並附 polymarket.com 連結（`--n` 預設 3） | 120–1500 | 3 |

每個範本都要求一個 H1 標題、結尾的 `## Sources`，並禁止「As an AI」之類的句子。

### 自訂範本

範本是一個 TOML 檔。`--template` 可以給內建名稱、`$AGENTBOX_HOME/templates/<名稱>.toml` 裡的使用者範本名稱，或直接給檔案路徑。最快的做法是先用 `agentbox report template show brief` 印出內建範本，改好再存起來。一個精簡的例子：

```toml
name = "vendor-scan"
description = "IT 採購：摘要、候選廠商（編號）、建議"
min_words = 150
citation_style = "inline-link"   # inline-link | footnote | numbered
min_sources = 3
sources_from_notes = true        # 引用的網址都必須來自 note 或 fetch 過的文件
forbid = ["As an AI"]

[[section]]
heading = "Summary"
max_words = 120

[[section]]
heading = "{n}. {title}"         # 編號段落：## 1. Foo、## 2. Bar ...
repeat_min = 3
repeat_max = 5
require_citation = true
keywords = ["vendor"]            # 帶有這些 tag 的筆記會放進這裡

[[section]]
heading = "Recommendation"
must_contain = ["budget"]
```

段落可用的欄位：`heading`、`level`（預設 2）、`required`、`repeat_min` / `repeat_max`（編號段落）、`min_words` / `max_words`、`require_citation`、`must_contain`、`table` 與 `columns`、`aliases`、`hint`（會寫進 TODO 裡）、`keywords`。打錯欄位名稱會直接報錯，不會默默忽略。

### 完整流程

從搜尋到交出一份通過檢查的報告，每一步都只是一個命令：

```bash
# 1. 搜尋，順手把前兩筆存成 doc
agentbox search "Rust web frameworks Axum Actix Rocket comparison" --max-results 3 --save 2
agentbox read doc:2 --grep Rocket

# 2. 記筆記；tag 用 item1、item2…（或段落關鍵字），build 時會放進對應段落
agentbox note add "Axum is async-first, built on Tokio" --source https://dev.to/... --tag item1
agentbox note add "Actix-web ~850K req/s baseline vs ~780K for Axum" --source https://reintech.io/... --tag item2
agentbox note add "Rocket 0.5 focuses on developer experience" --source https://reintech.io/... --tag item3

# 3. 產生骨架（先看預覽，再加 --apply 寫入）
agentbox report build "Top 3 Rust Web Frameworks in 2026" --template top-n --n 3 --out report.md --apply

# 4. 逐一填 TODO
agentbox file replace report.md --todo 1 --replace "Axum" --apply

# 5. 檢查
agentbox report check report.md --template top-n --n 3 --format md
```

假設模型不小心把第一項寫成 `### 1. Axum`，檢查結果會是：

```
## FAIL: `report.md` (template `top-n`), score 88/100

Stats: words 130 · sections 4 · citations 6 · sources 3 · todos_left 0

1 error(s), 0 warning(s):

- [ ] **error** `heading_level` (line 3): `### 1. Axum` must be a level-2 heading (`## `), found level 3
  - fix: `agentbox file replace "report.md" --find "### 1. Axum" --replace "## 1. Axum" --apply`
```

照著 fix 執行，再檢查一次就會是 `PASS ... score 100/100`。JSON 版（預設輸出）的每個 issue 長這樣：

```json
{"severity":"error","rule":"heading_level","line":3,"message":"`### 1. Axum` must be a level-2 heading (`## `), found level 3",
 "fix":"agentbox file replace \"report.md\" --find \"### 1. Axum\" --replace \"## 1. Axum\" --apply"}
```

會檢查的項目：留下的 TODO、多個 H1、標題格式（`##Foo` 少空格、setext 標題）、標題層級、編號（`1)`、`1:` 這類近似寫法、跳號、順序）、項目數量太多或太少、缺少的段落（名稱相近時直接給改名命令）、範本以外的段落（警告）、段落順序、每段字數與全文字數、段落沒有引用、引用格式不符（警告）、必須提到的關鍵字、缺表格或缺欄位、Sources 段落名稱與來源數量、引用了沒在 note / fetch 中出現過的網址（範本開啟 `sources_from_notes` 時）、禁用句。程式碼區塊裡的內容一律略過。分數是 100 減去每個錯誤 12 分、每個警告 4 分；只要沒有錯誤就算 `pass`。報告沒過關時命令本身仍然成功（`ok:true`、結束碼 0），要看的是 `data.pass`。

## 抽取資料：`extract`

`extract` 從 doc（或本機的 .md／.txt／.html 檔）抽出結構化項目，模型不用寫正規表示式或程式。完全是固定規則，工具裡沒有呼叫任何語言模型，同樣的輸入一定得到同樣的結果。

```bash
agentbox extract doc:3 --kind prices
agentbox extract doc:1 doc:2 doc:3 --kind prices --save-table      # 一次抽多份文件
agentbox extract --from doc:1,doc:2 --kind tables
agentbox extract doc:4 --kind links --site nvidia.com
agentbox extract doc:4 --kind dates --section 3 --grep founded --limit 10
```

| `--kind` | 抽出什麼 | 每個項目的主要欄位 |
|---|---|---|
| `tables` | Markdown 管線表格與殘留的 HTML `<table>`；每張表存成 `tbl:N` | `table`、`title`（所在段落標題）、`headers`、`rows_count`、`preview`（前 5 列） |
| `prices` | `$1,299/yr`、`US$ 49 per user/month`、`€9,99`、`NT$ 300 元`、`¥1.2萬`、`$12–19/user/mo` | `value`、`currency`（ISO 代碼）、`period`、`per`、`raw` |
| `dates` | `March 3, 2025`、`2025-03-03`、`3/4/2025`、`2023年5月1日`、`民國112年5月1日`、`Jan 2024` | `date`（ISO 格式）、`precision`（day／month）、`ambiguous`（`3/4/2025` 這種月日可能對調的寫法）、`raw` |
| `people` | 「Jane Doe, CEO of Acme」、「CEO Jane Doe」、「執行長王小明」等句型 | `name`、`role`、`org`、`confidence` |
| `links` | Markdown 連結與裸網址，一律轉成絕對網址；`--site` 只留指定網域 | `text`、`url`（連結本身）、`source`（文件網址） |
| `numbers` | 帶單位或量級的數字：`$68.1 billion`、`12.5%`、`3.2萬`、`500 GiB` | `value`、`unit`、`raw` |
| `emails` | 電子郵件地址（略過 `logo@2x.png` 這類圖檔名） | `email` |

共同規則：

- 每個項目都附 `doc`、`url`、`section`、`line` 與約 120 字的 `context`，引用時直接用；同一份文件內重複的項目會合併（例如 `March 3, 2025` 與 `2025-03-03` 只留一筆）。
- `--limit` 預設 20；超過時回 `truncated:true`，`hint` 會建議用 `--grep`、`--section` 縮小範圍或調高上限。
- `--save-table` 會把**全部**項目（不只顯示的那幾筆）存成一張 `tbl:N`，接著就能用 `table query` 排序、過濾。
- `$` 預設當成美元，但網站是 `.tw`、`.ca`、`.au`、`.hk` 等國家網域時會換成當地貨幣；`¥` 在 `.cn` 是人民幣，其他網站是日圓。判斷不了的情況請看 `raw` 與 `context`。
- `people` 是句型比對，不是真的人名辨識：每筆有 `confidence`，`hint` 也會提醒要回原文確認。
- `fetch` 會把網頁裡真正的資料表格轉成 Markdown 表格（連結保留、合併儲存格補空格），排版用的表格則保持文字，所以 `extract --kind tables` 抓得到大部分價目表與規格表。

## 表格：`table`

`table` 處理 `tbl:N` handle，也直接吃 CSV、TSV、JSON（物件陣列、二維陣列，或 agentbox 外層裡的第一個陣列）與 Markdown 表格檔。

```bash
agentbox table show tbl:1                                  # 欄位、推斷的型別（number/currency/date/text）、列數、前 20 列
agentbox table import prices.csv                           # 存成 tbl:N
agentbox table query tbl:1 --where "Price < 100" --where "Plan contains pro" --sort -Price --limit 5
agentbox table query tbl:1 --select Plan,Price --sort Price --format md
agentbox table query tbl:1 --group-by Vendor --agg "count,avg:Price,max:Price" --format csv
agentbox table query tbl:1 --where "Region = EU" --save    # 結果另存成新的 tbl:N
agentbox table export tbl:2 --out cheapest.csv             # 先預覽，加 --apply 才寫檔（.csv／.tsv／.md／.json）
```

`--where` 只有一種寫法：`欄位 運算子 值`，運算子是 `=`、`!=`、`<`、`<=`、`>`、`>=`、`contains`、`startswith`。可以重複，條件之間是 AND。比較時會自動判斷型別：

- 值看起來是數字時用數字比較，`$1,299/yr`、`US$ 49 per user/month`、`€9,99`、`1.2萬` 這類儲存格都會先轉成數字；資料量 `512 MiB`、`4 GiB`、`1 TB` 一律換算成 G 單位，所以 `Memory >= 2` 就是「至少 2 GB」。
- 值是日期時用日期比較（`Launched >= 2024-06-01`，儲存格可以是 `March 3, 2025` 或 `2023年5月1日`）。
- 其他情況是不分大小寫的文字比較。

`--sort -Price` 是由大到小，`--sort Price` 由小到大，可重複指定多個鍵；空白或無法比較的儲存格（像 `Contact us`）一律排在最後。`--agg` 支援 `count`、`sum`、`avg`、`min`、`max`，輸出欄位叫 `sum_Price` 這種名稱。執行順序固定是 where → group → sort → select。

欄位名稱不分大小寫，也容忍空白與符號差異（`price usd` 可以對到 `Price (USD)`）。打錯時錯誤訊息會列出所有欄位、最接近的名稱，以及改好的命令：

```json
{"ok":false,"error":{"code":"unknown_column","message":"no column named `Prise`; columns are: Plan, Price (USD), Seats"},
 "hint":"Did you mean `Price (USD)`? Try: --sort \"-Price (USD)\""}
```

全域的 `--format csv` 只對表格輸出（`table show`、`table query`）有效，會直接印出 CSV；其他命令維持 JSON。

在 `schema` 裡，表格功能拆成 `table_show`、`table_query`、`table_import`、`table_export` 四個工具（理由同報告工具：每個工具只列自己用得到的參數）；`extract` 則是一個工具加上 `kind` 列舉，因為各種 kind 的參數完全一樣。

### 從搜尋到比較報告

```bash
# 1. 搜尋並把前三筆存成 doc
agentbox search "Asana Monday ClickUp pricing per user per month" --max-results 4 --save 3

# 2. 一次從三份文件抽出價格，全部存成 tbl:1
agentbox extract doc:1 doc:2 doc:3 --kind prices --save-table

# 3. 只看「每人每月」的價格，由便宜到貴
agentbox table query tbl:1 --where "per = user" --where "period = month" --sort value --select value,raw,doc --format md

# 4. 產生比較報告骨架，再把表格與引用填進去
agentbox report build "Asana vs Monday vs ClickUp pricing" --template compare --columns "Tool,Entry price (USD/user/mo),Source" --out report.md --apply
```

第 3 步的實際輸出（2026 年 10 月）：

| value | raw | doc |
|---|---|---|
| 7 | $7/user/mo | doc:1 |
| 7 | $7/user/month | doc:2 |
| 9 | $9/user/mo | doc:1 |
| 10.99 | $10.99/user/mo | doc:1 |

## 股價：`quote`

`quote` 不需要金鑰：預設用 Yahoo Finance 的公開端點，Yahoo 擋下或限流時自動改用 Stooq。輸出的 `backend` 欄位會寫明資料來源。

```bash
agentbox quote get NVDA 2330.TW ^TWII USDTWD=X        # 一次查多檔，空白或逗號分隔都可以
agentbox quote history 2330.TW --range 6mo --save     # 每日 OHLCV + 摘要，全部列存成 tbl:N
agentbox quote history ^GSPC --range 5y               # 5y 預設週線，max 預設月線
agentbox quote search "Taiwan Semiconductor"          # 用公司名稱找代號
agentbox quote search 台積電                           # 常見台港股與指數的中文名稱也查得到
```

代號一律用 Yahoo 格式：

| 市場 | 範例 |
|---|---|
| 美股 | `NVDA`、`BRK-B` |
| 台股上市／上櫃 | `2330.TW`、`6488.TWO` |
| 港股 | `0700.HK` |
| 指數 | `^GSPC`、`^IXIC`、`^TWII`、`^HSI` |
| 匯率 | `USDTWD=X`、`JPYTWD=X` |
| 加密貨幣 | `BTC-USD`、`ETH-USD` |

- `quote get` 每檔回傳 `price`、`previous_close`、`change`、`change_pct`、`currency`、`exchange`、`type`、`market_state`（pre／regular／post／closed）、`time`（帶交易所時區的 ISO 時間）、`timezone`、當日高低、`volume`、`week52_high`／`week52_low`。一次最多 10 檔；部分代號失敗時其餘照常回傳，失敗的放在 `errors`。
- 只給數字（例如 `2330`）會回 `ambiguous_symbol`，hint 建議加上 `.TW`（上市）或 `.TWO`（上櫃）；不認得的代號回 `symbol_not_found`，hint 建議改用 `quote search`。
- `quote history` 的 `--range` 可選 `5d`、`1mo`、`3mo`、`6mo`、`ytd`、`1y`、`5y`、`max`，`--interval` 可選 `1d`、`1wk`、`1mo`。輸出 `summary`（起訖日期與收盤、漲跌與漲跌幅、區間最高／最低與日期）和最近 30 列；`--save` 會把全部列存成 `tbl:N`，可再用 `table query tbl:N --sort -volume` 之類的命令分析。
- Yahoo 回 429 或被擋時（`rate_limited`），hint 會建議 `--backend stooq`。Stooq 只補「最新報價」：沒有前一日收盤，所以 `change` 是 `null`；也沒有台股。Stooq 的歷史資料自 2026 年起需要免費金鑰，若要用它當歷史資料的備援，請設定環境變數 `STOOQ_API_KEY` 或執行 `agentbox config set stooq.api_key -`（從標準輸入讀取）；和 Tavily 金鑰一樣，不接受命令列旗標，也不會出現在任何輸出裡。
- `quote search` 先比對內建的別名表（台積電、鴻海、聯發科、騰訊、加權指數、比特幣、美元台幣等），再合併 Yahoo 的搜尋結果；Yahoo 的搜尋不接受中文，所以中文查詢只會回傳別名表裡有的項目。

> 報價可能延遲（通常約 15 分鐘），僅供研究參考，不適合拿來下單。每次輸出的 `note` 欄位也會提醒這一點。

## 預測市場：`market`

`market` 讀取 Polymarket 的公開資料（唯讀、免金鑰）：活動與市場來自 Gamma API，機率走勢來自 CLOB API。機率就是市場價格換算成的百分比（0.62 → 62%），是交易者的定價，不是預測。

```bash
agentbox market trending                              # 24 小時成交量最高的活動
agentbox market trending --tag politics --limit 5     # 只看某個主題（tag slug）
agentbox market search election --limit 15            # 關鍵字搜尋
agentbox market search "fed rate" --closed            # 已結算的市場
agentbox market get balance-of-power-2026-midterms    # 單一活動的所有市場、規則與網址
agentbox market history 2026-balance-of-power-d-senate-d-house-949 --interval 1m --save
```

- **搜尋**：先用 Polymarket 的公開搜尋端點抓最多 3 頁（每頁 25 個活動）當候選，再在本機做不分大小寫的關鍵字比對（每個字都要出現在活動標題、市場問題、說明或 tag 裡），去重後依 `--sort` 排序（`volume` 預設、`liquidity`、`end` 最快結束的在前、`newest`）。`--limit` 預設 10；過去經驗是候選太少會找不到相關市場，所以候選池刻意抓大。沒有任何活動同時包含所有關鍵字時，會改列 Polymarket 自己的相近結果，並標上 `match:"fuzzy"`。搜尋端點故障時，改掃成交最活躍的 300 個活動再本機過濾（`via:"events-scan"`）。
- **狀態**：預設只看進行中的活動；`--closed` 只看已結算的，兩個旗標都給就全部。`--tag` 是 tag 的 slug（例如 `politics`、`elections`、`crypto`、`sports`、`economy`），每個活動的 `tags` 欄位會列出自己的 slug。
- **輸出**：每個活動有 `title`、`slug`、`url`（`https://polymarket.com/event/<slug>`）、`end_date`、`volume`、`volume_24h`、`liquidity`、`tags`，以及機率最高的前 3 個市場。每個市場有 `odds`（例如 `Yes 63.5% · No 36.5%`），是非題另有 `yes_pct`，其他題型有 `leader` 與 `leader_pct`；`change_1d_pts` 是一天內變動的百分點。
- **`market get`** 接受活動或市場的 slug、數字 id，或直接貼 polymarket.com 網址，回傳所有市場（`--limit` 預設 20）、每個結果的機率、一週變動、最後成交價、結算規則（`description`）與每個市場的網址。Polymarket 預先建立、還沒有價格的占位市場（例如「Person X」）會自動略過。
- **`market history`** 要指定單一市場（市場 slug 或 id）。傳入有多個市場的活動時，會回 `ambiguous_market`，並列出機率最高的幾個市場 slug 供挑選。`--interval` 是 `1d`（每小時一點）、`1w`（每 6 小時，預設）、`1m`（每日）、`max`（每日）；`summary` 給起訖機率、`change_pts`（百分點變化）與區間高低點，時間一律 UTC。
- **存成表格**：`market search` 與 `market trending` 加 `--save-table`，`market history` 加 `--save`，結果都會存成 `tbl:N`。市場表格的欄位是 event、market、outcome、pct、odds、change_1d_pts、volume_24h、volume、end_date、url、market_id，例如 `agentbox table query tbl:3 --where "pct >= 50" --sort -volume_24h`。
- **連不上**：DNS 或連線失敗時回 `dns_error`／`network_error`，hint 會提醒：有些 DNS 過濾服務（例如採用 RPZ 封鎖清單的解析器）會擋 polymarket.com，可以用 `nslookup gamma-api.polymarket.com` 和公開解析器（如 1.1.1.1）的結果比對。

### Polymarket 簡報範例

內建的 `market-brief` 範本專門用在這類簡報：Overview 加上 N 個 `## n. 市場` 段落，每段都必須寫出機率（含 `%`）並附上 polymarket.com 連結，否則 `report check` 會報錯。

```bash
# 1. 找題目：熱門或關鍵字
agentbox market trending --tag politics --limit 5
agentbox market search "2028 presidential" --limit 15

# 2. 看細節（所有結果、規則、網址），必要時看走勢
agentbox market get balance-of-power-2026-midterms
agentbox market history 2026-balance-of-power-d-senate-d-house-949 --interval 1m

# 3. 記筆記，tag 用 item1、item2、item3，build 時會放進對應段落
agentbox note add "Democrats Sweep trades at 63.5%" --source https://polymarket.com/event/balance-of-power-2026-midterms --tag item1

# 4. 產生骨架、填 TODO、檢查
agentbox report build "Polymarket politics brief" --template market-brief --n 3 --out brief.md --apply
agentbox file replace brief.md --todo 2 --replace "Balance of Power: 2026 Midterms" --apply
agentbox report check brief.md --template market-brief --n 3 --format md
```

想用一般的編號清單也可以：把第 4 步的範本換成 `--template top-n --n 3`，只是不會檢查 `%` 與 polymarket.com 連結。

## 給 agent 用的 Skill

光有執行檔，模型不會知道該怎麼用。專案附了一份符合 [Agent Skills](https://agentskills.io/specification) 格式的說明（英文，寫給小模型看），放在 [`skills/agentbox/`](skills/agentbox/)：

- `SKILL.md`：何時使用、怎麼讀 `ok`／`data`／`hint`、標準研究流程（search → read → note → extract／table → report build → file replace → report check）、每個子命令一行的速查表、硬性規則與完成條件。刻意控制在 200 行以內，小模型的 context 才放得下。
- `references/`：需要時才讀的細節，包括搜尋技巧、抽取與表格語法、報告範本與修正方式、股價與 Polymarket、檔案編輯（含 heredoc 寫法），以及常見任務的做法（股價簡報、高階主管查詢、定價比較、Polymarket 簡報、歐盟法規、開源替代方案、IT 採購等）。
- `assets/vendor-shortlist.toml`：自訂報告範本的範例。

安裝方式（`agentbox` 執行檔必須已經在 PATH 上）：

```bash
agentbox skill install                 # 預覽：列出會寫入 ~/.agents/skills/agentbox/ 的檔案
agentbox skill install --apply         # 寫入；Windows 是 %USERPROFILE%\.agents\skills\agentbox
agentbox skill install --dir ~/my-openclaw-workspace/skills --apply   # 指定 OpenClaw workspace 的 skills 目錄
```

也可以直接複製資料夾：把 `skills/agentbox` 整個複製到 `~/.agents/skills/agentbox`，或 OpenClaw workspace 的 `skills/agentbox`。release 壓縮檔裡也附了同一份 `skills/` 資料夾。複製後開一個新的 agent session，skill 才會載入。OpenClaw 會依 `metadata.openclaw.requires.bins` 檢查 PATH 上有沒有 `agentbox`，找不到時不會載入這個 skill。

skill 裡的每一行 `agentbox ...` 範例都會在測試中用真正的命令列定義解析一次（`cargo test skill`），範例和實際旗標不一致時 CI 會失敗。

## 建置

需要 Rust 1.85 以上。

### Linux

```bash
cargo build --release
./target/release/agentbox --help

# 完全靜態的執行檔（需要 musl-tools）
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools
cargo build --release --target x86_64-unknown-linux-musl
```

### Windows

安裝 [rustup](https://rustup.rs/)（MSVC toolchain）後，在 PowerShell 執行：

```powershell
cargo build --release
.\target\release\agentbox.exe --help
```

也可以在 Linux 上交叉編譯 GNU 版本：

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```

推送 `v*` 標籤時，GitHub Actions 會自動建置 `x86_64-unknown-linux-musl` 與 `x86_64-pc-windows-msvc` 兩個版本，連同 `SHA256SUMS.txt` 附到 GitHub Release；名稱含 `-` 的標籤（例如 `v0.2.0-rc.1`）會建成草稿的預先發行版。手動執行 workflow（`workflow_dispatch`）只建置 artifacts，不會發布。各版本的變更見 [CHANGELOG.md](CHANGELOG.md)。

### 測試

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## agent 使用範例

一個「查某產品定價」的典型流程，模型每一步只需要下一個命令：

```bash
$ agentbox search "Acme Pro plan pricing" --max-results 3
{"ok":true,"data":{"backend":"tavily","query":"Acme Pro plan pricing","results":[{"rank":1,"title":"Pricing | Acme","url":"https://example.com/pricing","snippet":"..."}, ...]},"hint":"Open a result with `agentbox fetch URL`, ..."}

$ agentbox fetch https://example.com/pricing
{"ok":true,"data":{"doc":"doc:4","title":"Pricing","url":"https://example.com/pricing","chars":8123,"sections":6,
 "outline":[{"section":1,"heading":"Pricing","chars":420},{"section":2,"heading":"Pro","chars":1310}, ...]},
 "hint":"Read a section with `agentbox read doc:4 --section N`, or find facts with `agentbox read doc:4 --grep KEYWORD`."}

$ agentbox read doc:4 --grep "per month"
{"ok":true,"data":{"doc":"doc:4","keyword":"per month","total_matches":2,"snippets":[{"section":2,"heading":"Pro","matches":1,"text":"…Pro plan costs $20 per month…"}, ...]},"hint":"..."}

$ agentbox calc "20 * 12 * 0.8"
{"ok":true,"data":{"expr":"20 * 12 * 0.8","result":192,"text":"192"},"hint":null}

$ agentbox note add "Pro 年繳打八折，每年 $192" --source https://example.com/pricing --tag pricing
$ agentbox note list --tag pricing
```

修改檔案時先看 diff，再決定要不要寫入：

```bash
$ agentbox file replace config.toml --find "debug = true" --replace "debug = false"
{"ok":true,"data":{"path":"config.toml","applied":false,...,"diff":"--- config.toml (current)\n+++ config.toml (proposed)\n@@ -1,2 +1,2 @@\n name = demo\n-debug = true\n+debug = false\n"},
 "hint":"Preview only. Review the diff, then rerun the same command with --apply to write it."}

$ agentbox file replace config.toml --find "debug = true" --replace "debug = false" --apply
```

### 接進 agent 框架

```bash
agentbox schema --implemented-only > tools.json   # 工具清單在 data.tools，每個元素就是一個 function tool
agentbox call read '{"doc":"doc:4","section":2}'  # 模型呼叫工具時，原樣轉給 agentbox
agentbox call report_check '{"file":"report.md","template":"top-n","n":3}'
```

報告相關的工具刻意拆成 `report_build`、`report_check`、`report_templates`、`report_template_show` 四個，而不是一個帶 `action` 參數的大工具（`table_*`、`quote_*`、`market_*` 也一樣）：每個工具只有自己需要的參數，小模型比較不會填錯，也不必記得哪些參數搭配哪個動作。

在 Windows PowerShell 傳 JSON 參數時引號容易被吃掉，建議改用標準輸入：

```powershell
'{"doc":"doc:4","section":2}' | agentbox call read -
```

框架只要看 `ok` 判斷成功與否，把 `hint` 一起回給模型即可；行程結束碼成功為 0、執行失敗為 1、參數錯誤為 2。

## 授權

MIT，見 [LICENSE](LICENSE)。
