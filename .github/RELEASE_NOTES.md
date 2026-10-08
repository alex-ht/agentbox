agentbox 是給小型語言模型 agent 用的 busybox 式命令列工具：一個靜態執行檔，涵蓋網頁擷取與搜尋、計算、檔案與筆記、資料抽取、表格查詢、股價、Polymarket 預測市場、報告產生與檢查。每個命令都輸出一行 JSON（`ok`、`data`、`hint`），並可用 `agentbox schema` 直接接進支援 function calling 的 agent 框架。完整變更見 [CHANGELOG.md](https://github.com/alex-ht/agentbox/blob/{{VERSION}}/CHANGELOG.md)。

### 主要功能

- `fetch`／`read`／`search`：網頁轉 Markdown、依段落或關鍵字讀取；有 Tavily 金鑰用 Tavily，沒有就用免金鑰的 DuckDuckGo／Bing。
- `extract`／`table`：從文件抽出表格、價格、日期等資料，再篩選、排序、分組。
- `quote`：美股、台股、港股、指數、匯率、加密貨幣報價與歷史走勢（免金鑰）。
- `market`：Polymarket 搜尋、熱門、詳情與機率走勢（唯讀、免金鑰）。
- `report`：依範本產生報告骨架並逐項檢查，附上可直接執行的修正命令。
- `calc`、`now`、`file`、`note`、`config`、`schema`、`call`。

### 安裝

**Linux（x86_64，完全靜態，不依賴 glibc）**

```bash
curl -LO https://github.com/alex-ht/agentbox/releases/download/{{VERSION}}/agentbox-{{VERSION}}-x86_64-unknown-linux-musl.tar.gz
tar -xzf agentbox-{{VERSION}}-x86_64-unknown-linux-musl.tar.gz
chmod +x agentbox-{{VERSION}}-x86_64-unknown-linux-musl/agentbox
mkdir -p ~/.local/bin && mv agentbox-{{VERSION}}-x86_64-unknown-linux-musl/agentbox ~/.local/bin/
agentbox --version    # 找不到命令時，把 ~/.local/bin 加進 PATH：export PATH="$HOME/.local/bin:$PATH"
```

**Windows（x86_64，PowerShell）**

```powershell
$v = "{{VERSION}}"
Invoke-WebRequest "https://github.com/alex-ht/agentbox/releases/download/$v/agentbox-$v-x86_64-pc-windows-msvc.zip" -OutFile agentbox.zip
Expand-Archive agentbox.zip -DestinationPath .
$dir = "$env:LOCALAPPDATA\Programs\agentbox"
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item -Force "agentbox-$v-x86_64-pc-windows-msvc\agentbox.exe" $dir
# 加進使用者 PATH（新開的視窗才會生效）
[Environment]::SetEnvironmentVariable("Path", [Environment]::GetEnvironmentVariable("Path", "User") + ";$dir", "User")
```

執行檔已靜態連結 C 執行階段，不需要另外安裝 VC++ 可轉散發套件。下載後可用文末的 SHA256 驗證（Linux：`sha256sum -c SHA256SUMS.txt --ignore-missing`；PowerShell：`Get-FileHash agentbox.zip`）。

### 設定 Tavily 金鑰（選用）

沒有金鑰也能搜尋（改用 DuckDuckGo／Bing）。有 Tavily 金鑰時，用環境變數或從標準輸入存進設定檔；agentbox 刻意不提供金鑰的命令列旗標，任何輸出也只會顯示遮罩後的值。

```bash
export TAVILY_API_KEY="tvly-你的金鑰"          # Linux：寫進 ~/.bashrc 就會一直生效
agentbox config set tavily.api_key -            # 或存進設定檔：貼上金鑰後按 Enter
agentbox config get                             # 確認來源與遮罩後的值
```

```powershell
[Environment]::SetEnvironmentVariable("TAVILY_API_KEY", "tvly-你的金鑰", "User")   # Windows：新開的視窗生效
```

### 注意

- 股價可能延遲約 15 分鐘，僅供研究參考；`quote` 用的是 Yahoo Finance 的非官方端點，可能被限流。
- Polymarket 的機率是市場價格，不是預測；部分 DNS 過濾服務會封鎖 polymarket.com。
