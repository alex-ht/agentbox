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

`--format md` 會把同一個外層轉成 Markdown：純量欄位變成項目清單、物件陣列變成表格、多行字串原樣輸出、`diff` 包在 ```` ```diff ```` 區塊內，最後一行是 `> hint: ...`。

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
├── docs/
│   ├── 1.md      轉換後的內容
│   └── 1.json    中繼資料：url、title、content_type、fetched_at
└── notes.jsonl   筆記，每行一個 JSON：id、text、source、tag、created
```

## 4. 有副作用的命令

會改動使用者檔案的命令（`file write`、`file replace`，以及之後的 `report --out`）預設只回傳 diff 預覽與 `applied:false`；同一個命令加上 `--apply` 才寫入。`file replace` 要求 `--find` 恰好出現一次，出現 0 次或多次都會回錯並在 `hint` 說明怎麼修正（多次時可加 `--all`）。

## 5. 工具 schema

`src/schema.rs` 有一張手寫的規格表，`agentbox schema` 由它產生 OpenAI function tool 格式；`agentbox call` 也用同一張表把 JSON 參數轉回命令列。單元測試會逐一比對規格表與 clap 定義（參數名稱、是否必填、預設值、可選值），兩邊不一致就會失敗，避免 schema 與實際行為脫節。巢狀子命令的工具名稱用底線連接，例如 `file_replace`、`note_add`。
