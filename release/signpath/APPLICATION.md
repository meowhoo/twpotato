# SignPath Foundation 申請包（草稿，未送出）

這份只準備文字與步驟；**申請、建立帳號、送出都由使用者執行**。
依據：SignPath Foundation 條款 https://signpath.org/terms 與 GitHub 整合文件 https://docs.signpath.io/trusted-build-systems/github （2026-10-27 查閱；送出前以當時官網為準）。

## 1. 申請表欄位草稿

| 欄位 | 內容 |
| - | - |
| 專案名稱 | 地瓜輸入法（TwPotato） |
| 網址 | https://github.com/meowhoo/twpotato |
| 授權 | MIT（OSI 核准；程式碼與模型權重） |
| 簡短描述 | Windows 繁體中文注音輸入法（TSF），以本機執行的小型神經網路依前後文選字；不連網、不收集資料。 |
| 維護者 | meowhoo（個人） |
| 建置方式 | GitHub Actions（GitHub-hosted `windows-latest`），`.github/workflows/build.yml`：Rust 1.98.1（`rust-toolchain.toml`）建置 TSF DLL、引擎、安裝輔助程式，Tauri 2 設定程式；WiX 5.0.2 組 per-machine MSI |
| 要簽的檔案 | `twime_tsf.dll`、`twime-engined.exe`、`twime-setup.exe`、`twime-settings.exe`（全部由本儲存庫原始碼建置），以及 `TwPotato-<版本>-x64.msi` |
| 發布位置 | GitHub Releases（目前已有未簽章版本） |
| 隱私 | 不傳送任何資料（`PRIVACY.md`） |

英文版描述（表單若要求英文）：
> TwPotato is an open-source (MIT) Bopomofo/Zhuyin input method for Windows (Text Services Framework). A small
> on-device neural model picks characters from context. It makes no network connections and collects no data.
> All signed binaries are built from this repository by GitHub Actions on GitHub-hosted runners.

## 2. 條件核對表

| 條件 | 狀態 | 依據／待辦 |
| - | - | - |
| OSI 授權、無商業雙授權 | 符合 | `LICENSE`（MIT） |
| 無專有元件 | 符合（待確認 1） | Rust crates 與 Tauri 均開源（`THIRD-PARTY-NOTICES.md`）；WebView2 是系統元件，不隨附 |
| 無惡意或不受歡迎程式、非駭客工具 | 符合 | 輸入法，由 Windows TSF 機制載入；不偵測、不利用、不繞過任何安全機制 |
| 持續維護 | 符合 | 公開儲存庫的提交紀錄 |
| 已以待簽形式發布 | **待辦** | 使用者先在 GitHub Releases 發布一個未簽章版本 |
| 下載頁描述功能 | 符合 | `README.md`「功能」 |
| 只簽自己的程式 | 符合 | 四個 PE 檔與 MSI 都由本儲存庫原始碼建置；未簽任何上游二進位檔 |
| 自動化、可驗證的建置 | 符合（待確認 2） | GitHub Actions、GitHub-hosted runner；模型資產見待確認 2 |
| 告知系統變更 | 符合 | MSI 安裝即註冊輸入法；README 說明安裝位置與註冊 |
| 提供解除安裝 | 符合 | MSI 解除；README「解除安裝與使用者資料」 |
| 多重要素驗證 | **待辦** | 使用者在 GitHub 與 SignPath 開啟 MFA |
| 角色（作者、審查者、核准者） | 符合（單人） | README「程式碼簽章政策」：三者都是 @meowhoo |
| 首頁「Code signing policy」段落 | **待辦（文字已備）** | 見第 3 節；README 現有中文段落要換成第 3 節的版本（標題須含英文 "Code signing policy"） |
| 隱私聲明 | 符合 | `PRIVACY.md`；第 3 節的固定句 |
| 中繼資料（產品名稱、版本一致） | 符合 | 四個 PE 檔都有 VERSIONINFO：ProductName「地瓜輸入法」、CompanyName「meowhoo」、ProductVersion＝版本；`release/build-msi.ps1` 會檢查 |
| 接受技術限制（每次人工核准） | 符合 | 第 4 節流程 |

## 3. 首頁「Code signing policy」段落（核准後放進 README；也放在 Release 說明）

```markdown
## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

- Committers and reviewers: [@meowhoo](https://github.com/meowhoo)
- Approvers: [@meowhoo](https://github.com/meowhoo)

Only files built by this repository's GitHub Actions workflow from the public source code are signed; every signing
request is approved manually. The model weights and dictionary are fixed-hash release assets (`release/model.lock`)
that are verified before packaging.

Privacy policy: this program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it. See [PRIVACY.md](PRIVACY.md).
```

（核准前不可放第一行歸屬句；核准前的 README 維持「尚未簽章」的說明。）

## 4. CI 整合計畫（核准後才啟用）

兩次簽章請求，各需一次人工核准：

1. **PE 檔**：建置後把四個 PE 檔以 `actions/upload-artifact`（v4 以上，pinned SHA）上傳為一個 artifact → `signpath/github-action-submit-signing-request`（pinned SHA，v3）送出：`organization-id`、`project-slug`、`signing-policy-slug`（`release-signing`）、`artifact-configuration-slug`（`pe-files`）、`github-artifact-id`＝上傳步驟的 output、`wait-for-completion: true`、`output-artifact-directory` → 簽好的檔案放回 bin 目錄。
2. 以簽好的 PE 檔執行 `release/build-msi.ps1`（Binary 表裡的 `twime-setup.exe` 因此也是簽好的）與 `check-msi.ps1`。
3. **MSI**：上傳 MSI → 同一 action，`artifact-configuration-slug`（`msi`）、同樣以 `parameters` 傳入 `version`→ 取回簽好的 MSI，重算 `.sha256`，上傳 Release 草稿。

- API token 存在 GitHub Secrets（`SIGNPATH_API_TOKEN`），只給簽章 job；簽章 job 的 `permissions` 只開需要的部分。
- PE 檔的簽章請求以 `parameters` 傳入版本（workflow 的 Version 步驟已驗證 tag＝crate 版本），artifact configuration 用它限制 ProductVersion：

```yaml
      - id: sign-pe
        uses: signpath/github-action-submit-signing-request@<commit SHA> # v3
        with:
          api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
          organization-id: <SignPath organization id>
          project-slug: twpotato
          signing-policy-slug: release-signing
          artifact-configuration-slug: pe-files
          github-artifact-id: ${{ steps.upload-pe.outputs.artifact-id }}
          parameters: |
            version: "${{ env.VERSION }}"
          wait-for-completion: true
          output-artifact-directory: signed-bin
```
- 所有到簽章請求為止的 job 都跑在 GitHub-hosted runner（OSS 規定）。
- `build-msi.ps1` 的 `SIGN_CMD` 佔位維持給本機手動簽章用；CI 走上面的 action。

Artifact configuration 草稿（語法送出前以 https://docs.signpath.io/artifact-configuration 核對）：

```xml
<!-- pe-files: the uploaded artifact is a ZIP with the four PE files at its root -->
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <parameters>
    <parameter name="version" required="true" />
  </parameters>
  <zip-file>
    <pe-file path="twime_tsf.dll" product-name="地瓜輸入法" product-version="${version}"><authenticode-sign /></pe-file>
    <pe-file path="twime-engined.exe" product-name="地瓜輸入法" product-version="${version}"><authenticode-sign /></pe-file>
    <pe-file path="twime-setup.exe" product-name="地瓜輸入法" product-version="${version}"><authenticode-sign /></pe-file>
    <pe-file path="twime-settings.exe" product-name="地瓜輸入法" product-version="${version}"><authenticode-sign /></pe-file>
  </zip-file>
</artifact-configuration>

<!-- msi: subject / author are the MSI summary information (release/wix/TwPotato.wxs; checked by release/check-msi.ps1) -->
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <parameters>
    <parameter name="version" required="true" />
  </parameters>
  <zip-file>
    <msi-file path="TwPotato-${version}-x64.msi" subject="地瓜輸入法 TwPotato" author="meowhoo"><authenticode-sign /></msi-file>
  </zip-file>
</artifact-configuration>
```

## 5. 申請前要向 SignPath 確認的問題

1. **Tauri 依賴的授權例外**（MPL-2.0、Unicode-3.0 等，見 `ime/settings-app/THIRD-PARTY-NOTICES.md`）都是開源授權，應不構成「專有元件」；確認無誤。
2. **模型資產**：MSI 內含預先訓練好的模型權重與詞表（MIT，固定雜湊的 GitHub Release 資產，CI 下載後驗證 `release/model.lock`），訓練資料不公開。是否符合「從原始碼可驗證建置」？若不行，備案（使用者決定）：
   - 只簽四個 PE 檔，MSI 不簽；或
   - 改走個人 OV 程式碼簽章憑證。
3. 單人專案（作者、審查者、核准者同一人）是否可接受。

## 6. 使用者待辦（依序）

1. 建立公開儲存庫 `meowhoo/twpotato`、推送匯出目錄（等使用者說 go）。
2. GitHub 帳號開啟 MFA。
3. 上傳模型資產 Release（`model-r4c1u`，`twime-model-r4c1u.zip`），讓 CI 能建出 MSI；發布第一個**未簽章**的 Release（`v0.1.0`）。
4. 在 https://signpath.org/apply 送出申請（用第 1 節的文字），同時提出第 5 節的問題。
5. 核准後：SignPath 帳號開啟 MFA；在 SignPath 建立專案、連結 GitHub.com trusted build system、建立兩個 artifact configuration 與 signing policy；GitHub Secrets 加入 API token；把第 3 節段落放進 README；啟用第 4 節的 CI 步驟（另行審查後才改 workflow）。
