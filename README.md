# agentbox

給「小型語言模型 agent」用的瑞士刀：一個靜態連結的單一執行檔，像 busybox 一樣把日常研究與電腦工作常用的功能收在一起。Linux 與 Windows 都能跑。

小模型最常卡在「工具之間的膠水」：抓網頁後要自己寫 Python 清 HTML、要用 `grep | jq` 找數字、算個漲跌幅還得開 REPL。agentbox 把這些膠水做進工具裡，模型只要選對子命令、填對參數，就能完成像 PinchBench 那類任務：查股價、找活動、市場調查、Polymarket 簡報、查高階主管、深度研究、競品研究、找開源替代品、比價、IT 採購、歐盟法規、BYOK 最佳實務等。

> 目前版本：v0.1。部分子命令先佔位，見下方狀態表。

## 設計原則

- **子命令少而完整**：控制在 12–15 個，一律「動詞（-名詞）」命名，參數攤平。不需要管線、正規表示式或 jq；過濾、排序、加總都做成內建選項。
- **輸出格式固定**：預設輸出一行精簡 JSON，加上 `--format md` 可改成 Markdown。所有子命令都用同一個外層結構：
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
| `file read\|write\|replace` | 讀取文字檔（可指定行範圍）；寫入與精確取代會先給 diff，`--apply` 才寫入 | ✅ 可用 |
| `note add\|list` | 暫存筆記與來源清單，跨呼叫保留，方便最後引用 | ✅ 可用 |
| `search <query>` | 網路搜尋：有 Tavily 金鑰就用 Tavily，否則用免金鑰的 DuckDuckGo（被擋時改用 Bing）；`--save N` 可把前幾筆直接存成 doc | ✅ 可用 |
| `config set\|get\|unset\|path` | 管理設定（例如 Tavily API 金鑰），金鑰一律遮罩顯示 | ✅ 可用 |
| `schema` | 輸出所有子命令的 function tool schema | ✅ 可用 |
| `call <name> <json>` | 用 schema 名稱 + JSON 參數執行工具（給框架用） | ✅ 可用 |
| `extract <doc>` | 從文件抽出連結、表格、數字、日期 | 🚧 規劃中 |
| `table <source>` | CSV／表格的過濾、排序、加總 | 🚧 規劃中 |
| `quote <symbol>` | 股價與歷史價格 | 🚧 規劃中 |
| `market <query>` | 預測市場（如 Polymarket）賠率 | 🚧 規劃中 |
| `report <title>` | 把筆記整理成附來源的 Markdown 報告 | 🚧 規劃中 |

規劃中的子命令參數已經定好，呼叫時會回傳 `not_implemented`，並在 `hint` 裡給一個目前就能用的替代做法（例如改用 `fetch` 抓某個公開 API）。

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

推送 `v*` 標籤時，GitHub Actions 會自動建置 `x86_64-unknown-linux-musl` 與 `x86_64-pc-windows-msvc` 兩個版本並附到 GitHub Release。

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
agentbox schema --implemented-only > tools.json   # 每個元素就是一個 function tool
agentbox call read '{"doc":"doc:4","section":2}'  # 模型呼叫工具時，原樣轉給 agentbox
```

在 Windows PowerShell 傳 JSON 參數時引號容易被吃掉，建議改用標準輸入：

```powershell
'{"doc":"doc:4","section":2}' | agentbox call read -
```

框架只要看 `ok` 判斷成功與否，把 `hint` 一起回給模型即可；行程結束碼成功為 0、執行失敗為 1、參數錯誤為 2。

## 授權

MIT，見 [LICENSE](LICENSE)。
