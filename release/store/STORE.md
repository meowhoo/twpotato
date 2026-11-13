# Microsoft Store 上架準備（草稿，簡版）

使用者同意先寫簡版。不建立帳號、不送審；費用與政策以送出當時的 Partner Center 官網為準。

## 前提

- **程式碼簽章必須先完成**：Store 的 MSI/EXE 應用要求安裝程式與其中的執行檔簽章並鏈到受信任的根憑證（SignPath Foundation 核准後才有，見 `release/signpath/APPLICATION.md`）。
- 安裝程式網址：GitHub Release 的版本化 HTTPS 網址，例如 `https://github.com/meowhoo/twpotato/releases/download/v0.1.0/TwPotato-0.1.0-x64.msi`（每個版本固定不變）。
- 靜默安裝參數：`/qn /norestart`（Store 會以系統管理員權限執行 MSI；結束碼 3010＝需重新開機）。

## 商品頁文字

- 名稱：地瓜輸入法
- 簡短描述：依前後文整句選字的注音輸入法，全部在本機執行，不收集資料。
- 描述：地瓜輸入法是 Windows 的繁體中文注音輸入法。輸入注音後，本機的小型神經網路依前後文一次選出整句的字，也可以逐字改選候選字。提供中英切換、全形／半形標點、使用者詞庫（新增、搜尋、匯入、匯出）與設定程式。不連網、不收集任何資料。開放原始碼（MIT）。
- 功能重點：整句選字；逐字候選；中英切換鍵可設定；使用者詞庫；不連網、不收集資料；開放原始碼。
- 關鍵字：注音、輸入法、繁體中文、台灣、Bopomofo、Zhuyin、IME
- 分類：生產力工具（Productivity）
- 隱私政策網址：`https://github.com/meowhoo/twpotato/blob/main/PRIVACY.md`
- 支援網址：`https://github.com/meowhoo/twpotato/issues`
- 年齡分級問卷（草稿）：無暴力、性、賭博、粗話內容；無使用者互動或分享；不收集個人資料；不連網 → 預期為「3+／所有人」。
- 系統需求：Windows 11 x64（Windows 10 未測試）。

## 截圖清單

1. 記事本中組字與候選視窗（淺色主題）。
2. 同上（深色主題）。
3. 設定程式「詞庫」頁（新增詞彙、搜尋）。
4. 設定程式「輸入設定」頁。
5. 工作列語言指示器的「中」／「英」圖示與右鍵「設定…」選單。

## Partner Center 步驟（使用者執行）

1. 註冊個人開發人員帳號（身分證件與自拍驗證；依當時官網費用）。
2. 建立新應用 → 類型選 **MSI 或 EXE 應用**，保留名稱「地瓜輸入法」。
3. 套件：填安裝程式網址、架構 x64、語言 zh-TW、靜默參數 `/qn /norestart`；確認 MSI 已簽章。
4. 商品頁：貼上上面的文字、上傳截圖與圖示（`ime/settings-app/src-tauri/icons/icon.ico` 的 300×300 PNG 版本另行輸出）。
5. 填年齡分級問卷、隱私政策網址、定價（免費）與市場（台灣，其他市場自選）。
6. 送審；被退件時依審查意見修正後重送。
