# 地瓜輸入法 Linux 版

有兩個前端：IBus 引擎程式（`ibus/`）與 Fcitx5 外掛（`fcitx5/`），共用同一個引擎行程、同一份設定與詞庫。

- **支援範圍**：Ubuntu 24.04 與 26.04（amd64），各有一組 `.deb`。IBus 是 Ubuntu 預設桌面（GNOME）內建的框架，
  裝好就能在「設定」裡加入；Fcitx5 給已經在用 Fcitx5 的人。
- 其他發行版沒有套件，可以照「從原始碼建置」自行安裝，不在保證範圍。
- **Linux 第一版沒有設定程式，也沒有設定頁面**：設定與使用者詞庫是兩個文字檔，用文字編輯器改，見「設定與使用者詞庫」。
  Windows 版的 `\` 詞庫提示、長詞自動完成、組字中加詞，Linux 第一版也還沒有。

## 安裝

到發布頁下載對應系統版本的套件（檔名裡有 `ubuntu24.04` 或 `ubuntu26.04`；用 `lsb_release -rs` 看自己的版本）：

| 套件 | 內容 | 誰需要 |
| - | - | - |
| `twpotato` | 引擎與模型（約 106 MB） | 一定要裝 |
| `ibus-twpotato` | IBus 前端 | 用 IBus（Ubuntu 預設桌面）的人 |
| `fcitx5-twpotato` | Fcitx5 前端 | 用 Fcitx5 的人 |

三個套件的版本要相同，一起裝、一起升級。只需要裝自己用的那個前端；兩個都裝也可以。

下面的指令用 `*` 代替版本號，請在**只放了這一次下載的套件**的資料夾裡執行。資料夾裡還留著別的版本（例如上次下載的舊版）時，
`*` 會把它們一起選進去，可能裝到舊的那一個；這時請把檔名完整寫出來。

### IBus（Ubuntu 預設桌面）

```sh
sudo apt install ./twpotato_*.deb ./ibus-twpotato_*.deb
```

1. **登出再登入**（IBus 在登入時才讀取新的輸入法）。
2. 「設定 → 鍵盤 → 輸入來源」按「＋」→「中文（台灣）」→「地瓜輸入法」→ 加入。
3. 用 `Super`＋`Space` 切換輸入來源；頂端列的輸入來源指示會顯示「中」或「英」。

只裝這兩個套件不會裝上 Fcitx5。

### Fcitx5

```sh
sudo apt install ./twpotato_*.deb ./fcitx5-twpotato_*.deb
```

系統還沒有 Fcitx5 時，這個指令會把 `fcitx5` 一併裝上（連同它建議的前端模組與設定工具）。Ubuntu 不會因為裝了 Fcitx5 就自動
改用它，要自己切換：

1. 讓這個帳號改用 Fcitx5。兩個 Ubuntu 版本的做法不同：
   - **Ubuntu 24.04**：`im-config -n fcitx5`（要改回 IBus：`im-config -n ibus`），然後**登出再登入**。
   - **Ubuntu 26.04**：`im-config` 在 GNOME（Wayland）工作階段不再生效（實測），要自己設定環境變數並讓 fcitx5 隨登入啟動，
     然後**重新開機**：

     ```sh
     mkdir -p ~/.config/environment.d ~/.config/autostart
     printf 'GTK_IM_MODULE=fcitx\nQT_IM_MODULE=fcitx\nXMODIFIERS=@im=fcitx\n' > ~/.config/environment.d/fcitx5.conf
     cp /usr/share/applications/org.fcitx.Fcitx5.desktop ~/.config/autostart/
     ```

     （要改回 IBus：刪掉這兩個檔案，再重新開機。）
2. 執行 `fcitx5-configtool`（Fcitx5 的設定工具），在可用的輸入法清單裡搜尋「地瓜」，把「地瓜輸入法」加進目前的輸入法 → 套用。
3. 用 `Ctrl`＋`Space` 切換（Fcitx5 的預設）。

要用左 Shift 切換中英，還要改一個 Fcitx5 的預設快速鍵，見下面「左 Shift 單按不會切換中英」。

## 升級與移除

- **升級**：把新版的套件下載到一個新的資料夾（不要和舊版的檔案放在一起），用和安裝相同的指令裝上去（已經裝著的那幾個要一起給，
  版本必須一致），然後**登出再登入**。
  登出之前，舊的前端與引擎還在記憶體裡，可以繼續打字；登入後才換成新版。套件不會替你結束執行中的行程。
- **移除**：`sudo apt remove twpotato`（已安裝的前端套件相依於它，會一起被移除），然後登出再登入。
  IBus：也可以先在「輸入來源」把「地瓜輸入法」拿掉。
- 移除（包含 `apt purge`）**不會刪除**你的設定與詞庫（`~/.config/twpotato/`）；確定不要了再自己刪掉那個資料夾。
- 把系統從 24.04 升級到 26.04 之後，請改裝 `ubuntu26.04` 那一組套件（跟著系統升級的情形沒有測過）。

## 設定與使用者詞庫

兩個檔案都在 `~/.config/twpotato/`（有設 `XDG_CONFIG_HOME` 時是 `$XDG_CONFIG_HOME/twpotato/`），UTF-8 文字檔。
資料夾與檔案一開始不存在，要用時自己建立：`mkdir -p ~/.config/twpotato`。兩個檔案 IBus 與 Fcitx5 共用。

**存檔後不必重新啟動任何東西**：引擎每 2 秒檢查一次這兩個檔案，有變就重新讀取。各項何時生效見各小節。

### `settings.toml`

一行一個 `鍵 = 值`；`#` 之後是註解。沒寫的鍵用預設值。全部的鍵：

```toml
toggle_key = "shift"      # 切換中英的鍵："shift"（左右 Shift 都可以，預設）、"left_shift"、"right_shift"、"none"（不用 Shift 切換）
default_mode = "chinese"  # 一開始的模式："chinese"（預設）或 "english"
punctuation = "full"      # 中文模式的標點："full" 全形（預設，Shift＋, 打出「，」）或 "half" 半形（打出「,」）
symbol_table = true       # 按 ` 開小符號表：true（預設）或 false
```

- 字串值要加雙引號，`true`／`false` 不加。
- **整個檔案只要有一行格式不對（少了引號、值不在上面列的範圍、同一個鍵寫兩次），整份設定都不採用，全部回到預設值**；
  不會有錯誤訊息。改完沒有效果時，先檢查這個。不認得的鍵會被略過，不算錯。
- 何時生效（存檔之後）：

  | 鍵 | 何時生效 |
  | - | - |
  | `punctuation`、`symbol_table` | 約 2 秒內，正在用的輸入欄位也會跟著變，不必切換或重新登入 |
  | `toggle_key` | 約 2 秒後，**再打一個注音（或切換一次輸入欄位）起**生效：前端要等引擎下一次回覆才知道新的設定。存檔時若正在英文模式，要切換一次輸入欄位後才生效 |
  | `default_mode` | 只影響「之後新開始」的輸入狀態，已經在用的不變。IBus：登出再登入之後；Fcitx5：之後新建立的輸入情境（通常是新開的程式或視窗） |

### `userlex.tsv`（使用者詞庫）

一行一個詞，欄位用 **Tab** 分隔（不是空白）：

```
詞<Tab>讀音[<Tab>權重]
```

```
# 以 # 開頭的行是註解
地瓜球	ㄉㄧˋ ㄍㄨㄚ ㄑㄧㄡˊ
凱蒂	ㄎㄞˇ ㄉㄧˋ	20
```

- **詞**：1 到 8 個字。
- **讀音**：注音，每個字一個音節，音節之間用空白或 `-` 隔開；音節數要和字數相同。一聲不標，輕聲的 `˙` 寫在音節
  前面或後面都可以（`˙ㄉㄜ`、`ㄉㄜ˙`）。
- **權重**：可以不寫（建議）。要寫的話是 0 到 30 的數字，愈大愈優先。
- 打出完整讀音時，3 個字以上的使用者詞會排在第一個；2 個字的詞優先程度較低，可能仍被常用詞蓋過。
- 格式不對的行（沒有 Tab、音節數與字數不同、有引擎不認得的字或音節）會被**略過**，其他行照常生效，不會有錯誤訊息。
- 上限：50,000 個詞、檔案 8 MB、同一個讀音最多 32 個詞。
- 何時生效：存檔後約 2 秒內，正在用的輸入欄位也會生效；刪掉一行或整個檔案同樣約 2 秒內失效。

## 左 Shift 單按不會切換中英：要關掉 Fcitx5 的一個預設快速鍵

（只有 Fcitx5 需要；IBus 沒有這個問題。）

**現象。** 地瓜輸入法用「單按 Shift」切換中文／英文。Fcitx5 原廠設定把「單按左 Shift」當成它自己的快速鍵
（`AltTriggerKeys`，暫時切換到第一個輸入法，通常是鍵盤配置），這個鍵在送到輸入法之前就被 Fcitx5 拿走。結果：

- 單按**左** Shift：Fcitx5 把目前的輸入法暫時換成鍵盤配置（再按一次換回來），不是地瓜輸入法的中英切換；
  狀態列不會顯示「英」，組字中的行為也和 Windows 版不同。
- 單按**右** Shift：不受影響，是地瓜輸入法的中英切換。

地瓜輸入法不會自動更改你的 Fcitx5 設定。想讓左 Shift 也用來切換中英，請自行把這個快速鍵拿掉，方法擇一：

**用設定工具。** 執行 `fcitx5-configtool`（需要套件 `fcitx5-config-qt`；KDE 在「系統設定 → 輸入法」）→
「全域選項」（Global Options）→「快速鍵」（Hotkey）→ 找到「暫時在第一個與目前的輸入法之間切換」
（Temporarily switch between first and current Input Method），把裡面的「左 Shift」刪掉 → 套用。
（選項的中文名稱依 Fcitx5 版本的翻譯可能略有不同，以英文名稱為準。）

**直接改設定檔。** 先結束 fcitx5（它結束時會把設定寫回檔案，執行中改會被蓋掉），編輯
`~/.config/fcitx5/config`，在 `[Hotkey]` 區段把 `AltTriggerKeys` 設成空的，再啟動 fcitx5：

```ini
[Hotkey]
AltTriggerKeys=
```

如果檔案裡已經有下面這種寫法（Fcitx5 自己存檔時的格式），把整個區段刪掉，改成上面那一行：

```ini
[Hotkey/AltTriggerKeys]
0=Shift_L
```

想恢復原狀：在設定工具把「左 Shift」加回去，或把 `AltTriggerKeys=` 那一行刪掉。

## 已知限制與已知行為

第一版沒有的功能：

- 設定程式與設定頁面（改用上面的兩個文字檔）；組字中加詞；`\` 詞庫提示；長詞自動完成。
- 只測過 Ubuntu 24.04 與 26.04 的 GNOME 桌面。Ubuntu 26.04 的 GNOME 只有 Wayland 工作階段。

兩個前端相同的部分：

- 密碼欄位：只有應用程式把欄位標示為密碼或敏感輸入時，輸入法才保證不處理按鍵、不記前文。**一般終端機裡 `sudo`
  之類的密碼提示通常沒有這個標示，輸入法無從得知，請自行切到英文再輸入密碼。**
- 輸入法會參考同一個輸入欄位裡剛打過的字（只放在記憶體，不寫入磁碟）。Linux 上偵測不到「用滑鼠把游標移到別處」，
  這些前文只在焦點離開、應用程式要求重設、進入密碼欄位時清除，所以換了位置之後的頭幾個字可能仍參考先前的內容。
- 鍵位：注音照美式鍵盤的實體位置對應。應用程式有送按鍵的硬體碼時（一般的桌面應用程式都有），系統鍵盤配置是
  Dvorak 等非美式配置也一樣；沒有硬體碼時才照送來的字元判斷。只有會打出文字的鍵（字母、數字列、標點）照位置
  判斷，Esc、Enter、方向鍵、數字鍵盤等照鍵本身的意義。前提是應用程式送來的硬體碼可信：送的是文字鍵、卻附上不相符
  硬體碼的程式（少數螢幕鍵盤、遠端桌面）會被照硬體碼解讀。
- 切換到別的輸入法、或輸入欄位失去焦點時：已經可以送出的組字會直接送出；還沒打完的注音會被丟棄。
  例外：Fcitx5 因為欄位的能力旗標改變而自己把輸入法換走時（主要是欄位變成密碼欄位；也包含應用程式把欄位改成
  不接受輸入法的情形），組字一律丟棄，不會送進那個欄位。GNOME 的 Wayland 工作階段裡用 IBus 時另有規則，見下。
- 應用程式要求重設（例如用滑鼠把游標移到別處）時：輸入法不再參考先前打過的字。正在組字的內容怎麼處理，兩個前端
  不同：Fcitx5 一律丟棄不送出；IBus 見下。
- 引擎在輸入欄位取得焦點、選用地瓜輸入法時就開始載入（約 0.6 秒）。載入完成前打的鍵會先暫存，載入好之後依原來的
  順序交給引擎，不會遺失、不會變成英文字母；最多等 2 秒，引擎還沒好就把暫存的鍵原樣交給應用程式。
  例外（IBus）：暫存期間如果換了焦點，暫存的鍵會遺失（IBus 不再把它們送到原欄位）。只有登入後第一次使用、或引擎
  意外結束後重新載入的那不到 1 秒內才可能遇到。
- 引擎行程（`twime-engined`）意外結束時：正在組的那一段會遺失；接著打的鍵同樣先暫存，等新的引擎載入好再處理。
- 不支援內嵌組字的應用程式，組字會顯示在候選窗旁而不是文字裡；候選窗的外觀與位置由桌面（IBus 在 GNOME 上是桌面殼，
  Fcitx5 是它自己的面板）決定。

Fcitx5 的行為（框架的設定，不是地瓜輸入法能決定的）：

- 每個輸入欄位各自記住有沒有切到輸入法：換到另一個欄位或另一個程式時，可能要再按一次 `Ctrl`＋`Space`。想讓它們共用同一個
  狀態，改 Fcitx5 全域選項的 Share Input State（設定檔 `~/.config/fcitx5/config` 的 `[Behavior]` 區段：`ShareInputState=All`；
  和上面的快速鍵一樣，要在 fcitx5 結束時改檔）。

IBus 與 Fcitx5 不同的地方（都來自 IBus 本身的設計）：

- **GNOME 的 Wayland 工作階段（Ubuntu 的預設）**：所有 GTK 程式都經由 GNOME 桌面殼使用 IBus，而桌面殼在焦點移走後才送出的
  組字，會落在**新取得焦點的欄位**（實測）。所以在這裡：
  - 組字中按下輸入法不處理的鍵（Tab、F6、`Ctrl`＋某鍵、`Alt`＋`Tab` 等，這些鍵可能讓焦點移走）時，組字會**先送出到原來的欄位**，
    再把那個鍵交給程式；還沒打完的注音則丟棄。單按 `Ctrl`、`Alt` 等修飾鍵不會。
  - 用滑鼠點別的欄位、或程式自己把焦點移走時，**還沒送出的組字會被丟棄**（不會送進任何欄位）。要保留就先按 Enter。
  - 應用程式要求重設時同樣丟棄（與 Fcitx5 相同）。
- 其他情況（X11 工作階段、Qt 等自己連 IBus 的程式）：IBus 由用戶端決定組字的去留，輸入法只能事先告訴它「送出」或「丟棄」。
  我們照「失去焦點時引擎會怎麼做」來設定，所以應用程式要求重設時**已經可以送出的組字會被送出**（Fcitx5 是丟棄）；
  還沒打完的注音一樣丟棄。
- 候選窗：面板拿到的只有當頁的候選，頁碼與翻頁按鈕可能顯示不正確；用鍵盤翻頁（PageUp／PageDown）不受影響。
- IBus 的前端程式（`ibus-engine-twpotato`）若被強制結束：已經可以送出的組字由應用程式送出一次，還沒打完的注音
  丟棄；之後 IBus 不會自動
  把它叫回來，要重新選一次輸入來源（切到別的再切回來）才能繼續用。
- 同一台機器也裝了 Fcitx5 時：Fcitx5 預設會接手 IBus 的用戶端（它的 IBus 前端模組），這時實際在運作的是 Fcitx5
  那一邊的地瓜輸入法。
- 敏感欄位在 IBus 上的判斷：用途是密碼或 PIN，或應用程式設了「不要記錄輸入內容」的提示（private）。

## 檔案位置

| 內容 | 套件安裝 | 使用者模式（開發用） |
| - | - | - |
| 引擎、IBus 前端程式 | `/usr/lib/twpotato/` | `~/.local/share/twpotato/bin/` |
| 模型與詞表 | `/usr/share/twpotato/assets/` | `~/.local/share/twpotato/assets/` |
| Fcitx5 外掛 | `/usr/lib/x86_64-linux-gnu/fcitx5/twpotato.so`、`/usr/share/fcitx5/{addon,inputmethod}/twpotato.conf` | `~/.local/share/fcitx5/{addon,inputmethod}/twpotato.conf` |
| IBus 元件檔 | `/usr/share/ibus/component/twpotato.xml` | 另一個元件目錄（見下） |
| 設定、使用者詞庫 | `~/.config/twpotato/` | 同左 |
| 授權與聲明 | `/usr/share/doc/{twpotato,ibus-twpotato,fcitx5-twpotato}/` | — |

使用者模式的引擎存在時（`~/.local/share/twpotato/bin/twime-engined`），前端優先用它和它旁邊的資產，不用套件安裝的那一份。

## 授權

地瓜輸入法自己的程式是 MIT。兩個前端**動態連結**系統提供的 Fcitx5、IBus（libibus）與 GLib（都是 LGPL-2.1-or-later），
套件裡不含、也不散布這些函式庫；前端的二進位檔含有它們標頭檔中的行內函式與巨集。對應的聲明與 LGPL 全文在
`/usr/share/doc/fcitx5-twpotato/` 與 `/usr/share/doc/ibus-twpotato/`，字典、emoji 資料與 Rust 套件的聲明在
`/usr/share/doc/twpotato/`。

## 從原始碼建置（開發用，或其他發行版）

需要：`rust-toolchain.toml` 指定的 Rust 工具鏈、`cmake`、`pkg-config`、`gcc`、`g++`、`libfcitx5core-dev`、`libibus-1.0-dev`
（Ubuntu 的套件名）。模型資產不在原始碼裡，另外取得後放進資產目錄。

```sh
ime/linux/build.sh                                   # 引擎、libtwime_shell.a、twpotato.so、ibus-engine-twpotato
ime/linux/fcitx5/install-user.sh <cmake 建置目錄>     # 預設是 $CARGO_TARGET_DIR/linux/fcitx5
```

引擎與資產另外放在 `$XDG_DATA_HOME/twpotato/bin/twime-engined` 與 `$XDG_DATA_HOME/twpotato/assets/`
（`XDG_DATA_HOME` 預設是 `~/.local/share`）。安裝後重新啟動 fcitx5，在 Fcitx5 設定裡加入「地瓜輸入法」。

IBus（開發用）：建置目錄裡的 `ibus/twpotato.xml` 是元件檔，裡面寫的是建置目錄中執行檔的絕對路徑。把它放進一個
自己的目錄，用環境變數 `IBUS_COMPONENT_PATH=<那個目錄>` 啟動 `ibus-daemon`（要連同系統的元件一起用，就把
`/usr/share/ibus/component` 也用冒號接在後面），再到輸入來源設定加入「中文（台灣）→ 地瓜輸入法」。

測試：`ime/linux/fcitx5/test/run.sh`、`ime/linux/ibus/test/run.sh`（無畫面，說明在檔頭）。

### 打包（Ubuntu 24.04／26.04）

```sh
TWP_ASSETS=<模型資產目錄> ime/linux/package.sh        # 產生三個 .deb 與一份清單（manifest）
ime/linux/packaging/check-installed.sh              # 裝好之後：無畫面地經兩個框架各打一句，確認用的是安裝的檔案
```

在要支援的那個 Ubuntu 版本上執行，一次產生該版本的一組（24.04 與 26.04 各自建置，不通用）。版本號是
`<上游版本>-1~ubuntu<系統版本>.<打包修訂號>`，修訂號用 `TWP_REVISION` 指定（預設 1）。另外需要 `dpkg-dev`；不需要 root。
套件的 Maintainer 欄位用 `TWP_MAINTAINER="名稱 <信箱>"` 指定；沒指定時是佔位的 `meowhoo <maintainer@invalid>`，只適合測試用的建置。
要發布的套件加 `TWP_RELEASE=1`：遇到佔位信箱、或原始碼有未提交的修改，腳本會拒絕打包。
腳本會檢查：前端只動態連結允許的系統函式庫、套件裡沒有夾帶別人的函式庫、程式用到的 Rust 套件都列在
`THIRD-PARTY-NOTICES.md`。細節在 `package.sh` 檔頭。

## Arch Linux（社群自助，不在保證範圍）

沒有 Arch 的套件，也不上 AUR；以下是從原始碼建置、裝在自己家目錄的步驟。有人願意維護 AUR 套件的話，歡迎告訴我們，
這裡會連過去。

**已在容器驗證**（官方 `archlinux` 映像，無畫面；2026-11-12：Fcitx5 5.1.23、IBus 1.5.34、GLib 2.90.1、gcc 16.2.1、Rust 1.98.1）：下面「套件」「建置」「安裝」三段的指令照樣執行成功，
兩個前端的無畫面功能測試（`fcitx5/test/run.sh func`、`ibus/test/run.sh component func`）全部通過。
**未驗證**：在真實桌面啟用框架的步驟（最後一段），是依 Fcitx5 與 IBus 的官方文件寫的，沒有在 Arch 的桌面上試過。

套件：

```sh
sudo pacman -S --needed base-devel rustup cmake pkgconf fcitx5 fcitx5-configtool ibus
```

（只用其中一個框架時，另一個框架的套件仍然要裝：`build.sh` 會把兩個前端都建出來。）

建置（在原始碼的根目錄）：

```sh
rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)" --profile minimal
ime/linux/build.sh
```

安裝（使用者模式；`<資產>` 是另外取得的模型資產目錄，`T` 是 cargo 的建置目錄，預設 `ime/target`，`D` 是使用者的資料目錄）：

```sh
T=ime/target
D=${XDG_DATA_HOME:-$HOME/.local/share}
mkdir -p "$D/twpotato/bin" "$D/twpotato/assets"
install -m 755 "$T/release/twime-engined" "$T/linux/ibus/ibus-engine-twpotato" "$D/twpotato/bin/"
cp "<資產>"/* "$D/twpotato/assets/"
ime/linux/fcitx5/install-user.sh "$T/linux/fcitx5"      # Fcitx5：設定檔指向建置目錄裡的 twpotato.so，建置目錄要留著
```

啟用（未驗證）：

- Fcitx5：依 Fcitx5 官方文件設定環境變數並讓 fcitx5 隨桌面啟動，重新登入後在 `fcitx5-configtool` 加入「地瓜輸入法」。
- IBus：IBus 只讀系統的元件目錄。把 `$T/linux/ibus/twpotato.xml` 裡 `<exec>` 的路徑改成
  `$D/twpotato/bin/ibus-engine-twpotato` 的絕對路徑（保留前後的引號與 `--ibus`），以 root 複製到
  `/usr/share/ibus/component/twpotato.xml`，重新登入後在桌面的輸入來源設定加入「中文（台灣）→ 地瓜輸入法」。
