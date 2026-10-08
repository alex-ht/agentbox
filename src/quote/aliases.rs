//! A small built-in alias list so common Taiwan/HK names and Chinese queries
//! resolve even though Yahoo search only understands Latin text.

pub struct Alias {
    pub keys: &'static [&'static str],
    pub symbol: &'static str,
    pub name: &'static str,
    pub exchange: &'static str,
    pub kind: &'static str,
}

const fn a(
    keys: &'static [&'static str],
    symbol: &'static str,
    name: &'static str,
    exchange: &'static str,
    kind: &'static str,
) -> Alias {
    Alias {
        keys,
        symbol,
        name,
        exchange,
        kind,
    }
}

pub const ALIASES: &[Alias] = &[
    a(
        &["台積電", "台積", "台灣積體電路", "tsmc"],
        "2330.TW",
        "Taiwan Semiconductor Manufacturing Company Limited",
        "Taiwan",
        "Equity",
    ),
    a(
        &["台積電adr", "tsmc", "tsmc adr"],
        "TSM",
        "Taiwan Semiconductor Manufacturing Company Limited (ADR)",
        "NYSE",
        "Equity",
    ),
    a(
        &["鴻海", "鴻海精密", "hon hai", "foxconn"],
        "2317.TW",
        "Hon Hai Precision Industry Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["聯發科", "mediatek"],
        "2454.TW",
        "MediaTek Inc.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["台達電", "delta electronics"],
        "2308.TW",
        "Delta Electronics, Inc.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["廣達", "quanta computer"],
        "2382.TW",
        "Quanta Computer Inc.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["聯電", "umc"],
        "2303.TW",
        "United Microelectronics Corporation",
        "Taiwan",
        "Equity",
    ),
    a(
        &["日月光", "日月光投控"],
        "3711.TW",
        "ASE Technology Holding Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["中華電", "中華電信", "chunghwa telecom"],
        "2412.TW",
        "Chunghwa Telecom Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["富邦金"],
        "2881.TW",
        "Fubon Financial Holding Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["國泰金"],
        "2882.TW",
        "Cathay Financial Holding Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["大立光", "largan"],
        "3008.TW",
        "Largan Precision Co., Ltd.",
        "Taiwan",
        "Equity",
    ),
    a(
        &["環球晶", "globalwafers"],
        "6488.TWO",
        "GlobalWafers Co., Ltd.",
        "Taipei Exchange",
        "Equity",
    ),
    a(
        &["元大台灣50", "台灣50", "0050"],
        "0050.TW",
        "Yuanta Taiwan Top 50 ETF",
        "Taiwan",
        "ETF",
    ),
    a(
        &["騰訊", "tencent"],
        "0700.HK",
        "Tencent Holdings Limited",
        "HKSE",
        "Equity",
    ),
    a(
        &["阿里巴巴", "alibaba"],
        "9988.HK",
        "Alibaba Group Holding Limited",
        "HKSE",
        "Equity",
    ),
    a(
        &["輝達", "英偉達"],
        "NVDA",
        "NVIDIA Corporation",
        "NASDAQ",
        "Equity",
    ),
    a(&["蘋果"], "AAPL", "Apple Inc.", "NASDAQ", "Equity"),
    a(
        &["微軟"],
        "MSFT",
        "Microsoft Corporation",
        "NASDAQ",
        "Equity",
    ),
    a(&["特斯拉"], "TSLA", "Tesla, Inc.", "NASDAQ", "Equity"),
    a(
        &["加權指數", "台股加權", "台灣加權指數", "台股", "taiex"],
        "^TWII",
        "TSEC Weighted Index",
        "Taiwan",
        "Index",
    ),
    a(
        &["標普500", "標普", "s&p 500", "s&p500"],
        "^GSPC",
        "S&P 500",
        "SNP",
        "Index",
    ),
    a(
        &["那斯達克", "那斯達克指數", "nasdaq composite"],
        "^IXIC",
        "NASDAQ Composite",
        "Nasdaq GIDS",
        "Index",
    ),
    a(
        &["道瓊", "道瓊指數", "dow jones"],
        "^DJI",
        "Dow Jones Industrial Average",
        "DJI",
        "Index",
    ),
    a(
        &["費半", "費城半導體", "philadelphia semiconductor"],
        "^SOX",
        "PHLX Semiconductor",
        "Nasdaq GIDS",
        "Index",
    ),
    a(
        &["恆生指數", "恒生指數", "恆指", "hang seng"],
        "^HSI",
        "Hang Seng Index",
        "HKSE",
        "Index",
    ),
    a(
        &["日經", "日經225", "nikkei", "nikkei 225"],
        "^N225",
        "Nikkei 225",
        "Osaka",
        "Index",
    ),
    a(
        &["比特幣", "bitcoin"],
        "BTC-USD",
        "Bitcoin USD",
        "CCC",
        "Cryptocurrency",
    ),
    a(
        &["以太幣", "以太坊", "ethereum"],
        "ETH-USD",
        "Ethereum USD",
        "CCC",
        "Cryptocurrency",
    ),
    a(
        &["美元台幣", "美金台幣", "美元兌台幣", "usd/twd", "usdtwd"],
        "USDTWD=X",
        "USD/TWD",
        "CCY",
        "Currency",
    ),
    a(
        &["日圓台幣", "日幣台幣", "jpy/twd"],
        "JPYTWD=X",
        "JPY/TWD",
        "CCY",
        "Currency",
    ),
    a(
        &["黃金", "金價", "gold"],
        "GC=F",
        "Gold futures",
        "COMEX",
        "Future",
    ),
    a(
        &["原油", "油價", "crude oil"],
        "CL=F",
        "Crude Oil futures",
        "NY Mercantile",
        "Future",
    ),
];

/// Aliases matching a query: an exact key (any script), or a CJK key
/// contained in the query ("台積電股價" finds 台積電).
pub fn lookup(query: &str) -> Vec<&'static Alias> {
    let q = query.trim().to_lowercase();
    let compact: String = q.split_whitespace().collect();
    let mut out: Vec<&Alias> = Vec::new();
    for al in ALIASES {
        let hit = al.keys.iter().any(|k| {
            let kc: String = k.split_whitespace().collect();
            q == *k
                || compact == kc
                || (!k.is_ascii() && k.chars().count() >= 2 && compact.contains(&kc))
        });
        if hit && !out.iter().any(|o| o.symbol == al.symbol) {
            out.push(al);
        }
    }
    out
}
