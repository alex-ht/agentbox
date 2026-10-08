# 更新紀錄

這個檔案記錄 agentbox 每個版本的重要變更，格式參考 [Keep a Changelog](https://keepachangelog.com/zh-TW/1.1.0/)，版本號遵循 [語意化版本](https://semver.org/lang/zh-TW/)。

## [Unreleased]

## [0.2.0] - 2026-10-08

讓小模型 agent 真的會用 agentbox：附上 Agent Skill，並補上幾個讓模型少犯 shell 錯誤的功能。

### 新增

- **Agent Skill**：`skills/agentbox/`（agentskills.io 格式，英文）教小模型怎麼用 agentbox：`SKILL.md` 加上 `references/` 裡的細節與常見任務做法；`agentbox skill install [--dir] [--apply]` 安裝到 `~/.agents/skills/agentbox` 或指定目錄。release 壓縮檔也會附上這份資料夾。
- `file replace --todo N`：直接取代報告骨架裡的第 N 個 `<!-- TODO(N): ... -->`；`report check` 的修正命令也改用這個寫法。
- `file write --content -`、`file replace --replace -`：從標準輸入讀內容，搭配 heredoc 就不必處理 `$`、引號與換行。
- `note clear [--tag] [--apply]`：開始新任務前清掉舊筆記。

### 測試

- skill 裡每一行 `agentbox ...` 範例都會用真正的 clap 定義解析，避免說明檔與命令列脫節。

## [0.1.0] - 2026-10-08

第一個公開版本：單一靜態執行檔，提供 Linux（x86_64 musl）與 Windows（x86_64 MSVC）版本。

### 新增

- **共同介面**：所有命令都輸出一行 JSON 外層 `{"ok","data","hint"}`，失敗時附錯誤碼與下一步建議；`--format md` 輸出 Markdown，`--format csv` 輸出表格。結束碼成功為 0、執行失敗為 1、參數錯誤為 2。
- **網頁**：`fetch` 把網頁轉成 Markdown 存成 `doc:N` 並回傳大綱；`read` 依段落、關鍵字或位移讀取。
- **搜尋**：`search` 有 Tavily 金鑰時用 Tavily，沒有時改用免金鑰的 DuckDuckGo／Bing；支援網域篩選、新聞、時間範圍與 `--save` 直接存成 doc。
- **工具**：`calc` 計算算式，`now` 顯示任意時區的現在時間。
- **檔案與筆記**：`file read|write|replace` 先顯示 diff，加 `--apply` 才寫入；`note add|list` 記錄附來源的筆記。
- **抽取資料**：`extract` 從 doc 或本機檔案抽出表格、價格、日期、人物、連結、數字、電子郵件，全部是固定規則，不呼叫語言模型。
- **表格**：`table show|query|import|export` 處理 `tbl:N` 與 CSV／TSV／JSON／Markdown 表格，支援篩選、排序、分組彙總，並自動辨識金額、容量與日期。
- **股價**：`quote get|history|search` 免金鑰查詢美股、台股上市櫃、港股、指數、匯率與加密貨幣；預設用 Yahoo Finance，被擋時改用 Stooq。
- **預測市場**：`market search|get|trending|history` 唯讀查詢 Polymarket 的活動、機率（百分比）、成交量與機率走勢。
- **報告**：`report build|check|templates|template show` 依 TOML 範本產生報告骨架，並逐項檢查、附上可直接執行的修正命令；內建 `top-n`、`compare`、`brief`、`market-brief` 等範本。
- **設定**：`config set|get|unset|path` 管理 Tavily、Stooq 金鑰；金鑰只從環境變數或標準輸入讀取，任何輸出都只顯示遮罩後的值。
- **接進 agent 框架**：`schema` 輸出 26 個 OpenAI 風格的 function tool，`call` 用工具名稱與 JSON 參數執行；單元測試確保 schema 與命令列定義一致。

[Unreleased]: https://github.com/alex-ht/agentbox/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/alex-ht/agentbox/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/alex-ht/agentbox/releases/tag/v0.1.0
