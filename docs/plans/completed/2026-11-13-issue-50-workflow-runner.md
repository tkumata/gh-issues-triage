# Issue #50: Release workflow の macOS runner 修正

## Status

- Phase: awaiting_external_verification
- Approval: approved
- 承認日: 2026-11-13
- Issue: `tkumata/gh-issues-triage#50`
- Branch: `chore/issue-50`
- 調査日: 2026-11-13
- 変更対象は本書と `.github/workflows/release.yml` とする。
- 以下の実装要件と受け入れ条件は必須 (MUST) とする。

## Goal

Release workflow の macOS ビルドを開始できる状態へ戻す。
サポートが終了した runner の指定を修正する。

Issue のタイトルは「gh workflows 修正」である。
Issue の本文は次のとおりである。

> The configuration 'macos-14-arm64-us-default' is not supported というエラーで止まるので調査して修正。

## Scope

- `.github/workflows/release.yml` の macOS runner を変更する。
- 変更は `macos-14` から `macos-15` への置換とする。
- 本書に検証結果と未確認の範囲を記録する。

## Non-goals

- Rust ソース、依存関係、Cargo のバージョンを変更しない。
- Linux runner、成果物名、公開条件、権限を変更しない。
- Release の再設計、Action の更新、検証基盤の追加を行わない。
- push、マージ、workflow 起動、Release 公開を自動実行しない。

## Required Behavior

### 確認済みの事実

- ローカルの開始時点は `6d991cc6c3b0db6f28e18e134d7ecbb6845dee95` である。
- 作業開始時の作業ツリーに変更はなかった。
- 調査時の Release workflow は macOS に `macos-14` を指定していた。
- 対応する成果物の platform は `macos-aarch64` である。
- 2026-11-13 の実行 `43363588344` では `prepare` が成功した。
- 同実行では Linux のビルドと成果物アップロードが成功した。
- 同実行では macOS ジョブが `cancelled` で終了した。
- macOS ジョブの step は 0 件である。
- macOS ジョブの annotation は Issue と同じエラーを示した。
- `release` ジョブはスキップされた。
- GitHub は 2026-11-02 に macOS 14 runner を廃止した。
- GitHub の公式移行先には arm64 の `macos-15` が含まれる。

### 修正方針

- 原因は、廃止済み runner の指定と判断する。
- macOS の runner は `macos-15` に固定する。
- 固定値は、OS の移行範囲を限定するために使用する。
- macOS の成果物は arm64 のまま維持する。
- macOS の成果物名は `gh-issues-triage-v<VERSION>-macos-aarch64.tar.gz` を維持する。
- ビルドコマンドは `cargo build --release --locked` を維持する。
- Linux の指定は `ubuntu-24.04-arm` を維持する。
- 既存のバージョン判定とタグ存在確認を維持する。
- 既存の成果物アップロードと Release 公開の処理を維持する。
- runner の更新だけでは、配布先 macOS の動作確認済みとは扱わない。

## Tasks

### Task 1: macOS runner の指定を修正する

- Status: completed
- Objective: 廃止済み runner による開始前の停止を解消する。
- Scope: `.github/workflows/release.yml` の macOS matrix 要素。
- Acceptance Criteria:
  - macOS の runner が `macos-15` である。
  - platform が `macos-aarch64` である。
  - workflow の差分が runner の置換 1 か所だけである。
  - `actionlint .github/workflows/release.yml` が成功する。
- Verification:
  - workflow の差分を確認する。
  - `actionlint .github/workflows/release.yml` を実行する。
  - `make check` と `make build` を実行する。
  - `git diff --check` を実行する。
  - 各コマンドの結果を本書へ記録する。
  - ローカル検証では GitHub runner の起動を確認できないと明記する。
- Result:
  - macOS runner を `macos-15` へ変更した。
  - workflow の差分は runner の置換 1 か所だけである。
- Validation (2026-11-13):
  - `actionlint .github/workflows/release.yml`: 成功。
  - `make check`: 成功。
  - `cargo fmt --check` と Clippy は成功した。
  - テストは 68 件成功、失敗 0 件、無視 1 件である。
  - 無視されたテストは実 TypeSafe API を使用する。
  - `make build`: 成功。
  - `git diff --check`: 成功。
  - ローカル検証では GitHub runner の起動を確認していない。

### Task 2: 修正反映後の GitHub 実行を確認する

- Status: pending
- Objective: 実際の runner と成果物の成功を確認する。
- Scope: 修正を含む Release workflow の実行結果の読み取り。
- Acceptance Criteria:
  - macOS ジョブが runner を取得し、step を開始する。
  - macOS のビルド、圧縮、アップロードが成功する。
  - Linux のビルド、圧縮、アップロードも成功する。
  - Release に両 platform の成果物が添付される。
- Verification:
  - Task 1 完了後、修正を含む実行の URL と commit を確認する。
  - ジョブ結果と Release の成果物名を読み取る。
  - 対象の実行がない場合、Task 2 は未確認として残す。
  - workflow ファイルだけの push は、現在の自動起動条件を満たさない。
  - 必要な workflow 起動や公開操作は、別途指示を受けて実行する。
  - 既存タグがある場合、`prepare` が公開をスキップすると考慮する。
- Result:
  - 修正はローカルの未コミット変更である。
  - 修正反映後の GitHub 実行は未確認である。
  - push、workflow 起動、Release 公開は実行していない。

## Open Questions

- Task 1 の実装を妨げる未決定事項はない。
- Task 2 の対象実行と実施時期は未決定である。

## References

- [Issue #50](https://github.com/tkumata/gh-issues-triage/issues/50)
- [停止した Release workflow](https://github.com/tkumata/gh-issues-triage/actions/runs/43363588344)
- [macOS ジョブと annotation](https://github.com/tkumata/gh-issues-triage/actions/runs/43363588344/job/131682655774)
- [GitHub の macOS 14 runner 廃止告知](https://github.blog/changelog/2026-10-01-github-actions-macos-14-runner-image-retirement/)
- [GitHub runner の公式一覧](https://github.com/actions/runner-images/blob/main/README.md)
- [Release workflow](../../../.github/workflows/release.yml)
- [設計の検証境界](../../current/DESIGN.md#検証境界)
