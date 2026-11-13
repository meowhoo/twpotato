# 隱私政策（地瓜輸入法 TwPotato）

最後更新：2026-10-27

地瓜輸入法**不收集、不傳送任何資料**。

- 所有選字運算都在你的電腦上執行；程式不連線到網路，沒有遙測、當機回報、廣告或自動更新。
- 你輸入的文字只在本機的輸入法元件與引擎之間傳遞（同一個 Windows 帳號內的具名管道，只允許該帳號連線）。
- 預設不寫任何記錄檔。只有在你（或開發者）手動設定診斷用的環境變數 `TWIME_TRACE` 時，才會在指定的檔案寫入事件名稱、按鍵代碼與時間；按鍵代碼可能透露輸入內容，診斷完請刪除該檔並移除這個變數。
- 本機保存的檔案都在 `%LOCALAPPDATA%\TwPotato`：設定 `settings.toml`、使用者詞庫 `userlex.tsv`、它們上一版的備份（`*.bak`），以及引擎的狀態檔（`running.lock`、`unconfirmed.json`，不含輸入內容）。這些檔案依 Windows 的帳號與檔案權限保護（預設只有你的帳號與系統管理員能存取）；解除安裝時不會刪除，你可以隨時手動刪除。
- 設定程式的匯入與匯出只讀寫你選擇的檔案。
- 模型由維護者事先訓練後隨程式發布；你的輸入不會被拿去訓練模型（本程式沒有自動學習功能）。

問題或回報：https://github.com/meowhoo/twpotato/issues

## English

TwPotato collects and sends no data. All processing happens on your computer; the program makes no network
connections (no telemetry, crash reporting, ads or auto-update). Typed text only passes between the input-method
component and the engine through a per-user named pipe. Nothing is logged by default; only if you (or a developer)
set the diagnostic environment variable `TWIME_TRACE` are event names, key codes and times written to the file it
names — key codes can reveal what was typed, so delete that file and the variable afterwards. Local files live in
`%LOCALAPPDATA%\TwPotato`: settings, user dictionary, their previous-version backups (`*.bak`) and engine state files
(no typed text). They are protected by normal Windows account and file permissions, kept on uninstall, and can be
deleted at any time. Your typing is never used to train the model.
