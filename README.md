# 地瓜輸入法（TwPotato）

給台灣人用的注音輸入法。打一整串注音，它會看前後文，一次把整句的字選好，同音字不用一個一個挑。

- **免費開源**：MIT 授權，程式碼與模型都公開。
- **完全離線**：所有運算都在你的電腦上，不連網、不收集任何資料（見 `PRIVACY.md`）。
- **支援平台**：Windows 11 x64、Ubuntu 24.04／26.04（IBus、Fcitx5）。

## 特點

- **整句選字**：連續打一整句注音，同音字（例如 在／再、的／得）由前後文自動決定；選錯的字可以逐字改。
- **台灣用語**：模型用台灣的文章訓練，用字、用詞以台灣習慣為準；常見的口語讀音（例如「們」讀 ㄇㄣˊ）也打得出來。
- **輕聲可直接打**：例如 ㄇㄛ˙、ㄌㄜ˙。
- **大千標準鍵盤**；中英切換鍵可選左／右 Shift；中文標點全形或半形。
- **使用者詞庫**：可新增、搜尋、匯入、匯出自訂詞。
- **emoji 候選**、小符號表；自動完成（預設關閉）。

## 準確度

用約 3 萬句沒有前文的短句測試，每打 1,000 個字的錯字數（越低越好）：

| 輸入法 | 每千字錯字 | 整句全對 |
|---|---:|---:|
| 地瓜輸入法 0.1.9 | 44.2 | 86.5% |
| 新酷音 | 57.3 | 83.1% |
| 微軟注音 | 62.8 | 81.9% |

測試方法、資料隔離與限制見 `MODEL_CARD.md`。另一組測試顯示，有前文時（接著前一句打）錯字約再少三成。

## 速度與資源

- 每次轉換最慢也在 5 毫秒內（p99 4.61 ms，x64 桌機），打字不會卡。
- 記憶體約 200 MB；磁碟約 200 MB（模型與字典約 187 MB）。

## 安裝

### Windows 11

1. 到 [Releases](https://github.com/meowhoo/twpotato/releases/latest) 下載 `TwPotato-<版本>-x64.msi`，執行安裝（需要系統管理員）。
2. 開「設定 → 時間與語言 → 語言與地區」，在「中文（繁體，台灣）」→「語言選項」→「鍵盤」加入「地瓜輸入法」。
3. 用 Win＋空白鍵切換到地瓜輸入法就可以打字。

目前安裝檔還沒有程式碼簽章，Windows 會顯示「未知的發行者」，按「其他資訊 → 仍要執行」即可。

### Ubuntu 24.04／26.04

到 [Releases](https://github.com/meowhoo/twpotato/releases/latest) 下載與系統版本相符的 `twpotato` 和前端套件（Ubuntu 預設桌面用 `ibus-twpotato`），在下載的資料夾執行：

```sh
sudo apt install ./twpotato_*.deb ./ibus-twpotato_*.deb
```

登出再登入後，在「設定 → 鍵盤 → 輸入來源」加入「中文（台灣）→ 地瓜輸入法」。Fcitx5 與其他細節見 [`ime/linux/README.md`](ime/linux/README.md)。

---

以下是進階說明。

## 功能細節

- 標準（大千）注音鍵盤；整句選字，依前後文決定同音字；可逐字改選候選字。
- 中英切換鍵（Shift 任一側／左 Shift／右 Shift／不使用）、預設模式、中文標點全形／半形、小符號表。
- 使用者詞庫：設定程式中新增、搜尋、刪除、匯入、匯出自訂詞（含讀音與權重）。
- 專有名詞詞表（人名、地名等）協助選字；一般詞庫（約 111 萬詞）只放進候選清單，不改變首選。
- 輕聲讀音可直接輸入（例如ㄇㄛ˙）；常見的口語讀音與教育部標準讀音都能打出來。
- emoji 候選；自動完成（候選清單補完詞，在設定程式中開啟，預設關閉）。
- 工作列語言指示器顯示「注」「中」「英」圖示。

## 系統需求

- Windows 11 x64（Windows 10 x64 應可使用，但尚未測試）。
- 約 200 MB 磁碟空間（模型與字典約 187 MB）；引擎實測最大工作集約 196 MB。
- 設定程式使用 Windows 內建的 Microsoft Edge WebView2 Runtime（Windows 11 已內建）。

## 詳細安裝說明（Windows）

### MSI（建議，全機安裝，需要系統管理員）

1. 從 GitHub Releases 下載 `TwPotato-<版本>-x64.msi`，以 `.sha256` 檔核對雜湊。
2. 執行 MSI。檔案裝在 `C:\Program Files\TwPotato\`，開始功能表會有「地瓜輸入法 設定」。
3. 到「設定 → 時間與語言 → 語言與地區」，在「中文（繁體，台灣）」的鍵盤選項中加入「地瓜輸入法」；之後用 Win＋空白鍵切換。

目前的發布版本**尚未程式碼簽章**，下載與執行時 Windows SmartScreen 會顯示「未知的發行者」警告（見下方「程式碼簽章政策」）。

靜默安裝與解除（系統管理員命令列）：

```powershell
msiexec /i TwPotato-<版本>-x64.msi /qn /norestart /l*v install.log
msiexec /x TwPotato-<版本>-x64.msi /qn /norestart
```

結束碼 3010 表示需要重新開機才能完成：仍載入舊 DLL 的程式（例如 explorer、開著的記事本）或開著的設定程式，其檔案會在重開機時替換或刪除。

### 使用者模式（不需系統管理員，開發與測試用）

`twime-setup install` 把檔案裝在 `%LOCALAPPDATA%\Programs\TwPotato`，只對目前帳號生效。全機（MSI）安裝存在時會拒絕安裝。

### 從使用者模式改用 MSI

安裝 MSI 時，會在同一個安裝交易中自動移除**執行安裝的帳號**自己的使用者模式安裝：程式檔案、該帳號的 COM 與 TSF 註冊、開始功能表捷徑。設定、詞庫（`%LOCALAPPDATA%\TwPotato`）與「語言」設定中已加入的輸入法都保留。安裝失敗時會回滾，使用者模式照常可用。

- 同一台電腦的**其他帳號**若也裝了使用者模式，MSI 不會動它們，請在那些帳號自己執行 `twime-setup uninstall`；以另一個系統管理員帳號的帳密提權安裝，或以 SYSTEM 部署時，同樣不會遷移。
- 使用者模式的引擎還有任何程式正在使用（開著並用過地瓜輸入法的視窗，包括正在組字）時無法遷移，安裝會失敗並保持原狀；請先關閉這些程式（或在其中切換到其他輸入法），再安裝。
- 安裝前已開著的程式仍載入舊的使用者模式元件，要重新開啟（或登出再登入）才能繼續輸入中文。
- 遷移的每一步記錄在 `%LOCALAPPDATA%\Temp\twpotato-migrate.log`。
- 安裝成功但舊檔案被鎖住而沒刪完時，登出再登入後在「設定 → 應用程式」對地瓜輸入法執行「修復」即可清完。

**使用者模式遷移未完成**（MSI 顯示「上次把使用者模式安裝改為全機安裝的作業沒有完成」）：先查看 `reg query HKCU\Software\TwPotato\Migrate` 的 `state` 與記錄檔。

- 全機輸入法已可使用、且 `HKCU\Software\Classes\CLSID\{C2DD0C9B-F6E9-489D-9ADF-F97658502774}` 不存在時，刪除殘留即可：

  ```powershell
  reg delete "HKCU\Software\Classes\CLSID\{C2DD0C9B-F6E9-489D-9ADF-F97658502774}.twpotato-migrating" /f
  reg delete "HKCU\Software\Microsoft\CTF\TIP\{C2DD0C9B-F6E9-489D-9ADF-F97658502774}.twpotato-migrating" /f
  Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\地瓜輸入法 設定.lnk.twpotato-migrating" -ErrorAction SilentlyContinue
  Remove-Item -Recurse "$env:LOCALAPPDATA\Programs\TwPotato"
  reg delete HKCU\Software\TwPotato\Migrate /f
  ```

- 要回到使用者模式（`state` 為 `restore-failed`）：把兩個 `.twpotato-migrating` 鍵複製回原名再刪除（`reg copy "<…>.twpotato-migrating" "<原名>" /s /f`，再 `reg delete "<…>.twpotato-migrating" /f`）、把捷徑改回原名、刪除 `%LOCALAPPDATA%\Programs\TwPotato\uninstalling`，最後 `reg delete HKCU\Software\TwPotato\Migrate /f`。

## Linux（Ubuntu 24.04／26.04）

Linux 版有 IBus 與 Fcitx5 兩個前端，以 `.deb` 打包（`twpotato`、`ibus-twpotato`、`fcitx5-twpotato`，附在 Release；也可用 `ime/linux/package.sh` 自行建置）。安裝、移除、
設定檔格式與已知限制見 [`ime/linux/README.md`](ime/linux/README.md)。Linux 第一版沒有設定程式：設定與使用者詞庫是
`~/.config/twpotato/` 下的 `settings.toml` 與 `userlex.tsv`，用文字編輯器修改。

## 解除安裝與使用者資料

從「設定 → 應用程式」解除「地瓜輸入法」，或使用上面的 `msiexec /x`。
使用者資料（`%LOCALAPPDATA%\TwPotato` 的設定 `settings.toml` 與詞庫 `userlex.tsv`）**不會刪除**；確定不再需要可手動刪除該資料夾。

## 提權程式與已知限制

- **以系統管理員身分執行的程式**：可以打中文，按鍵會送到同一帳號、一般權限的引擎；提權程式本身不會啟動引擎。若還沒有任何一般程式用過地瓜輸入法，提權程式中只能英文直通，切到任一般程式打一個字後即可使用。
  - 殘留風險：已在同一帳號內執行的惡意程式可以注入引擎，影響輸入到系統管理員程式的文字（但無法借用提權權杖）。微軟不把 UAC 視為安全邊界；不想承擔的人可以在提權程式中改用其他輸入法。
- **以其他帳號提權**（輸入另一個管理員帳密）或開啟 Windows「Administrator Protection」時，提權程式只能英文直通。
- **Microsoft Store／UWP 等 AppContainer 程式與低完整性程式**：連不到引擎，只能英文直通。
- 登入畫面、服務：不提供中文輸入。
- 升級或解除後，之前已開著的程式仍載入舊 DLL，要重新開啟（或登出再登入）才能繼續輸入中文。
- 安裝或解除時引擎會被強制結束：正在組字的內容會消失，尚未寫入的設定或詞庫變更可能遺失（已寫入的檔案不會半寫）。
- 解除安裝失敗而回滾時，輸入法會重新註冊，但可能要在語言設定中重新加入。

## 建置

需要：Rust 1.98.1（`rust-toolchain.toml`）、Visual Studio Build Tools（MSVC、Windows SDK）、Node.js 20.10.0、.NET SDK 8 與 WiX Toolset 5.0.2（`dotnet tool install --global wix --version 5.0.2`）。

```powershell
cd ime
cargo test --workspace --locked
cargo build --release --locked -p twime-tsf -p twime-engined -p twime-setup
cd settings-app; npm ci; npm run build; cd src-tauri; cargo build --release --locked; cd ..\..\..
```

模型資產（`model.bin` 等）由維護者產生，以 GitHub Release 資產 `twime-model-<版本>.zip` 提供；`release/model.lock` 記錄檔名、大小與 SHA-256：

```powershell
& release\pack-model.ps1 -Zip twime-model-r9.zip -OutDir assets
& release\build-msi.ps1 -Version 0.1.9 -BinDir <四個執行檔所在目錄> -AssetsDir assets -OutDir out
& release\check-msi.ps1 -Msi out\TwPotato-0.1.9-x64.msi -Version 0.1.9 -AssetsDir assets
```

GitHub Actions（`.github/workflows/build.yml`）用同一組腳本建置、測試並產生 MSI；需要私有資料的測試在缺檔時會自動略過。訓練資料與訓練程式不公開。

Linux（Fcitx5／IBus，`ime/linux/`）與 macOS（InputMethodKit，`ime/macos/`）的前端程式碼也在本儲存庫，共用同一個引擎；Release 附 Windows 安裝包與 Linux `.deb`；macOS 版尚未發布。

## 程式碼簽章政策

- 目前的 Release **未簽章**。我們已準備申請 [SignPath Foundation](https://signpath.org) 的免費開源程式碼簽章；核准後，Release 中的 DLL、EXE 與 MSI 都會由 SignPath 簽章，憑證發給 SignPath Foundation。
- 只簽由本儲存庫的 GitHub Actions 從公開原始碼建置的檔案；每次簽章都要由維護者在 SignPath 人工核准。
- 角色：作者、審查者、核准者均為 [@meowhoo](https://github.com/meowhoo)；GitHub 與 SignPath 帳號都啟用多重要素驗證。
- 模型權重與詞表不是由原始碼建置，而是固定雜湊的 Release 資產（`release/model.lock`），建置時驗證雜湊後才打包。
- 隱私：本程式不連網、不收集資料（`PRIVACY.md`）。

## English

TwPotato is a Bopomofo (Zhuyin) input method for Windows 11 x64 (Windows 10 untested) that picks characters for a whole phrase from
context with a small on-device neural model. MIT licensed (code and model weights); the training data is not
published. Install the per-machine MSI from GitHub Releases (unsigned for now, so SmartScreen will warn), then add
"地瓜輸入法" under Chinese (Traditional, Taiwan) in the Windows language settings. Elevated programs work through the
user's normal-privilege engine; Store/UWP (AppContainer) apps get English pass-through only. Nothing is collected or
sent anywhere. See `MODEL_CARD.md`, `PRIVACY.md` and `THIRD-PARTY-NOTICES.md`.
