<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat のロゴ" width="180">
</p>

<h1 align="center">OOOSplat</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue?style=flat" alt="License: Apache-2.0"></a>
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.7.1"><img src="https://img.shields.io/badge/release-v0.7.1-blue?style=flat" alt="Release: v0.7.1"></a>
  <a href="#システム要件"><img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue?style=flat" alt="Platforms: Windows, Linux, macOS"></a>
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.7.1"><strong>ダウンロード</strong></a> | <a href="README.md">中文</a> | <a href="README_EN.md">English</a> | <a href="README_JA.md">日本語</a>
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/daily?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Day" width="250" height="55" /></a>
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/weekly?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Week" width="250" height="55" /></a>
</p>

OOOSplat は、通常の動画やパノラマ動画、画像シーケンスを 3D Gaussian Splatting のプロジェクトに変換するローカルのデスクトップアプリケーションです。素材、プロジェクトのディレクトリ、品質プリセットを選ぶだけで、生成、プレビュー、編集、書き出しまで行えます。

Windows と Apple Silicon 向けの macOS Alpha では、FFmpeg、FFprobe、COLMAP、Brush がアプリケーションに同梱されています。Linux のサポートは、引き続き Ubuntu 24.04 LTS x86_64 の Alpha に限られます。生成のすべての段階はユーザー自身の CPU と GPU で動作し、入力素材、プロジェクトのデータ、モデル、ログを、クラウドの再構成サービスや学習サービスにアップロードする必要はありません。React のインターフェースはローカルの Rust バックエンドを直接呼び出します。通常の生成とプレビューに、別の HTTP サーバーは必要ありません。

予定している作業については [OOOSplat のロードマップ](ROADMAP_EN.md)を参照してください。

> 0.7.1 では、生成のデフォルト設定、自動リトライ、Detailed のスプラット数の予算を更新し、段階的なフレームサンプリングと移動速度の調整を追加し、再撮影、画像の GPS 補助、書き出し、エラー診断を改善しました。

以下の機能は現在の `main` のソースツリーについて説明したものです。インストーラーの内容は各リリースのノートを確認してください。

## OOOSplat を選ぶ理由

- **ワンクリックで Gaussian を生成**：入力の動画または画像シーケンスのフォルダー、プロジェクトのディレクトリ、品質プリセットを選びます。あとは OOOSplat が、画像の準備、COLMAP によるカメラの再構成、Brush による学習、`final.ply` の公開までを実行するので、エンジンの手動設定やコマンドラインでの手順の組み立ては必要ありません。
- **クロスプラットフォーム対応**：Windows、macOS、Linux に対応し、主要な 3 つのデスクトッププラットフォームすべてでローカルでの Gaussian Splat の生成が可能です。OS とプロセッサーの具体的な要件は、下記の互換性のセクションを参照してください。
- **セキュリティとプライバシーの保護**：元の素材、抽出したフレーム、カメラの再構成データ、Gaussian のモデル、ログは、デフォルトでユーザーが選んだローカルのプロジェクトディレクトリにとどまります。中核となる生成のワークフローはユーザーのマシン上で動くため、元の動画、画像、モデルを第三者の再構成や学習のプラットフォームにアップロードする必要はありません。これにより、ネットワーク転送、クラウドでの保持、不正アクセスによる流出のリスクを減らせます。
- **計算処理はすべてローカル**：再構成と学習は、リモートの計算サービスを使わずにユーザー自身のマシン上で動きます。COLMAP は、互換性のあるローカルの NVIDIA GPU があれば自動的にそれを使い、なければ CPU にフォールバックするので、処理とデータの両方をユーザーの管理下に置けます。

## デモ動画

https://github.com/user-attachments/assets/5b9e8cef-4c71-4bfa-ba23-641fcdd37659

## インターフェースのプレビュー

### タスクの作成と管理

![OOOSplat のタスク作成とタスク一覧のワークスペース](assets/screenshots/task-workspace.png)

### Gaussian Splat のプレビューと調整

![OOOSplat の Gaussian Splat のプレビューと変形のワークスペース](assets/screenshots/gaussian-preview.png)

## 主な機能

- **ワンクリック生成**：MP4/MOV の動画、または JPG、JPEG、PNG の画像フォルダーから Gaussian Splat を作成します。画像の準備、カメラの再構成、学習、PLY の公開は OOOSplat が行います。
- **パノラマ素材**：つなぎ合わせ済みの単眼の、完全な 360°×180°、2:1 の ERP の MP4 と画像フォルダーに対応します。素材の種類は自動で判別され、判別しきれない入力はその場で選択を求めます。
- **画像の順序の確認**：100 枚を超える通常またはパノラマのフォルダーでは、生成の前に撮影順を確認します。順序のあるセットにはシーケンシャルマッチングを使い、順序のないセットと 100 枚以下のセットには総当たりのマッチングを使います。
- **画像の GPS**：埋め込まれた位置のメタデータを検出し、検証済みの Windows x64 の経路でマッチングと再構成を補助します。信頼できる観測があれば、推定した北の向きと実寸のスケールが得られます。そうでなければ元のワークフローを使います。[範囲と検証](docs/IMAGE_GPS.md)
- **外部モデルの取り込み**：Completed グループから、標準／圧縮 PLY、SOG、SPZ、SPLAT、KSPLAT を取り込めます。完全な COLMAP のデータは自動で検出され、条件を満たすプロジェクトは互換性の確認のあとで再撮影できます。外部のファイルは読み取り専用のままです。
- **3 つの品質プリセット**：再構成の最適化とフレームの補完は標準で有効です。動画のサンプリングは段階的に調整でき、手動でサンプリングした実行では追加のフレームは最大 20% までです。Detailed は元の学習解像度を保ち、通常の素材のスプラット数の予算を、有効なピクセル数と再構成のトラックから決めます（係数 0.5）。パノラマには別の予算があります。
- **自動リトライ**：デフォルトで有効です。GPU の OOM や再撮影プランのフォールバックのあとに、スプラット数の上限を下げてやり直せるようにします。Completed の詳細には、実際に行われたリトライが表示されます。
- **透明な素材**：透明な MOV と PNG のファイルは元の色とアルファを保ったまま、OOOSplat が COLMAP に必要な白黒のマスクを作成します。
- **マルチタスクのワークスペース**：タスクを New Tasks、Completed、Unfinished に分けて管理します。複数の下書きを残し、並べ替え、次のタスクを自動で実行できます。同時に実行される生成タスクは 1 つだけで、タスクはキャンセルや再開ができます。
- **再撮影**：再構成のモードと元のモデルを再利用するかどうかを選び、通常の学習レシピで学習します。サイズの異なる通常の入力の混在と、完全な ERP の再撮影に、別の派生プロジェクトで対応します。
- **プレビューと編集**：WebGPU/WebGL2、移動の軸、回転のリング、切り抜き、削除、反転、元に戻す、やり直しに対応します。静止したビューでは再描画を止め、操作中は最大 60 FPS で動きます。水平方向の WASD を 0.1×〜5.0× の速度で使ったり、最初の学習ビューに戻ったりできます。
- **複数の書き出し形式**：標準 PLY、圧縮 PLY、SPZ v4、バンドル版の SOG、縦長または横長の MP4、スタンドアロンのオフライン HTML に書き出せます。SPZ の範囲外の座標は、確認のあとで書き出し用のコピーに対して調整できます。
- **ローカルでの処理**：COLMAP による再構成と Brush による学習はユーザーのコンピューター上で動き、利用できる場合は GPU による高速化を使います。
- **クロスプラットフォームと 2 か国語**：Windows、Apple Silicon の macOS、Ubuntu に対応し、簡体字中国語と英語のインターフェースを備えています。

パノラマの各バッチには、同じサイズの画像が少なくとも 2 枚必要です。2:1 の比率だけでは ERP とは確定できません。デュアル魚眼、ステレオ、VR180、切り抜かれたパノラマ、非公開の元素材、通常とパノラマを混ぜた再撮影には対応していません。パノラマのキャッシュには 20 GiB の固定の上限がないので、十分なディスク容量を確保してください。画像 1 枚あたりのデコードの上限は引き続き 512 MiB です。検証の限界については、[動画](docs/PANORAMA_VIDEO_V1.md)、[画像](docs/PANORAMA_IMAGES_V1.md)、[再撮影のノート](docs/RESHOOT_V2.md)を参照してください。

圧縮 PLY、SPZ、SOG の書き出しは、推定メモリ量に基づいてモデルを拒否することはなくなりましたが、実際の RAM、ディスク容量、形式の制約は引き続き適用されます。これらの制限をなくしても、大きなモデルの書き出しが成功するとは限りません。[書き出しのノートと検証](docs/model-export-verification.md)を参照してください。

### Gaussian の編集のショートカット

- 矩形選択では、マウスの左ボタンでドラッグすると選択を置き換え、`Shift + drag` で追加、`Ctrl + drag` で除外します。マウスの中ボタンで周回、右ボタンでパン、ホイールでズームします。
- 黄色のハイライトは一時的な選択を示します。`Delete` または `Backspace` を押すと選択した Gaussian を非破壊で削除し、`Esc` で一時的な選択を解除します。
- 矩形、球、ボックスの選択では、Invert selection を選ぶか `Ctrl + I`（macOS では `⌘ + I`）を押すと、選択または保持する点を切り替えます。
- 球とボックスの選択では、自動で正投影のビューになります。側面、正面、上面のビューを切り替え、ビューポートか数値の欄で範囲を調整します。
- Transform パネルの「Scene gizmo」で Move と Rotate を切り替えます。選択中のボタンをもう一度クリックするとギズモがオフになります。軸やリングを左ドラッグするとモデルを編集し、何もないところをドラッグするとカメラが周回します。リングと数値による回転は、どちらもモデルの中心を軸にします。ボタンを離すと 1 回の編集として確定し、`Esc` で現在のギズモのドラッグを取り消します。ギズモのモードはローカルに保存されます。
- `WASD` で水平面上を移動し、`Shift` で 3 倍の速度になります。編集が完了するとキャンバスにフォーカスが戻ります。右下のスライダーで速度を 0.1× から 5.0× まで調整でき、このデバイスでの設定が記憶されます。速度はズームに連動し、透視投影のビューでのみ使えます。
- `Ctrl + Z` で元に戻し、`Ctrl + Shift + Z` または `Ctrl + Y` でやり直します。変形、切り抜き、削除の変更は 1 つの履歴を共有し、カメラの移動と一時的なハイライトは記録されません。

## 処理パイプライン

```text
Input video or image sequence
  │
  ├─ Video: inspect with FFprobe and extract at the preset target FPS or manual ratio and working resolution; emit RGBA frames and masks for transparent media
  ├─ Images: scan headers and naturally sort filenames, copy source media, prefer hard links for internal reuse, and create Alpha masks when needed
  ├─ COLMAP: auto-select CPU/CUDA for features; sequential video matching, exhaustive images by default, ordered matching above 100 images, and preset loop detection
  ├─ COLMAP: incremental reconstruction and registration validation, with bridge-frame recovery; two-view triangulation only for actual transparent images
  ├─ Panoramas: retain full ERP images for spherical reconstruction, then convert registered images to six-face training data
  ├─ Brush: train Gaussian Splats with an available GPU backend; Detailed plans training at source resolution
  └─ Validate the PLY and atomically publish final.ply
```

通常の生成タスクは、COLMAP が少なくとも 1 枚の登録済み画像と有効な 3D 点を生成すれば Brush に進みます。登録率が 80% を下回ると品質の警告が出ますが、カバー率が低いという理由だけでタスクを止めることはありません。再撮影では、新たに追加した画像のうち少なくとも 1 枚が登録される必要もあります。

画像シーケンスの事前チェックでは、JPG/PNG のヘッダーだけを読んで枚数、寸法、アルファチャンネルを確認し、すべてのピクセルをデコードすることはしません。アルファチャンネルがあっても、透明なピクセルを含むとは限りません。生成時に実際の透明度を確認し、必要な場合だけマスクを有効にします。プロジェクトには引き続き独立した元素材のコピーが残ります。内部の画像は、対応していればハードリンクを使い、そうでなければコピーにフォールバックし、取り込みと準備の間は進捗とログを表示します。

## システム要件

- Windows 11、x64。
- WebView2 Runtime への対応。
- 動画の書き出しには、WebView2 で WebCodecs の AVC に対応している必要があります。エンコードを使えない場合でもアニメーションモードは引き続き使え、書き出しが無効になっている理由を UI が表示します。
- Brush の学習に使える GPU のグラフィックスバックエンド。単体の GPU を推奨します。
- COLMAP の CUDA による高速化には、NVIDIA の GPU、Windows のドライバー 580.00 以降、Compute Capability 7.5 以上が必要です。これらの要件を満たさない場合、OOOSplat は自動的に CPU を使います。手動での設定は必要ありません。
- 元素材のコピー、入力画像、COLMAP のデータ、Brush の中間ファイル、最終的な PLY のための十分なディスク容量。長い動画、大きな画像シーケンス、高い品質プリセットでは、かなりの容量が必要になることがあります。
- インストーラーはマシン単位でインストールするため、管理者権限が必要になる場合があります。

Windows に同梱されている COLMAP のビルドは、CPU と CUDA の GPU による実行の両方に対応しています。OOOSplat は各タスクの前に利用できるバックエンドを自動で選びます。Brush は独自に利用できるグラフィックスバックエンドを使い、その GPU の検出と実行は COLMAP から独立しています。

### macOS 15+ Alpha（Apple Silicon のみ）

> 現在の配布物は、M1 以降の Apple Silicon の Mac 向けの、署名も公証もされていない `.app`/`.dmg` の Alpha です。Intel の Mac と Universal Binary には対応していません。

- ネイティブの arm64 の FFmpeg 8.1.2、本物のスタンドアロンの FFprobe、COLMAP 4.2.1 の Ceres CPU の CLI 専用版、OOOBrush ooo-v1.0.0 の CLI を同梱しています。
- ユーザーが Homebrew をインストールする必要はなく、OOOSplat が Homebrew やシステムの `PATH` にあるエンジンにフォールバックすることもありません。
- この Alpha では、COLMAP は常に CPU を使います。Brush は利用できる Metal のグラフィックスバックエンドを独自に選び、UI がこの違いを説明します。
- 初回起動時に、署名のない Alpha が Gatekeeper にブロックされることがあります。アプリの入手元と完全性を確認したうえで、[Apple の開き方の説明](https://support.apple.com/en-us/102445)に従い、システム設定 → プライバシーとセキュリティの「このまま開く」を使ってください。[macOS 15 以降では、Control キーを押しながらクリックして開く方法は使えなくなりました](https://developer.apple.com/news/?id=saqachfa)。

### Ubuntu 24.04 Alpha（x86_64 のみ）

> この Alpha では、Ubuntu 24.04 でビルドした x86_64 の `.deb` パッケージを提供します。Ubuntu 22.04、他の Linux ディストリビューション、本番環境へのデプロイへの対応はうたっていません。

- Ubuntu 24.04 LTS、x86_64。
- Brush が対応しているグラフィックスバックエンドとドライバー。Brush は公式に AMD、Intel、NVIDIA の GPU に対応しています。現在のエンドツーエンドの検証には NVIDIA を使いました。CPU のみのソフトウェアのグラフィックスバックエンドは未検証ですが、起動時のチェックで人為的にブロックしてはいません。
- ソースからのビルドには、Node.js 22.12+、Rust stable、Tauri 2 の WebKitGTK の開発用依存関係が必要です。`.deb` のユーザーはこれらの開発ツールを必要としません。
- Ubuntu のシステムの `ffmpeg`/`ffprobe`。自前でビルドしハッシュで固定した COLMAP 4.2.1 を同梱しており、apt や PATH から選ばれることはありません。
- OOOBrush ooo-v1.0.0 の Linux x86_64 の CLI。`npm run setup:engines` でインストールと検証を行います。

Ubuntu の依存関係は次のコマンドでインストールします：

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file ffmpeg \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
```

グラフィックアダプターに合った動作する Vulkan のドライバーをインストールしてください。たとえば、NVIDIA のプロプライエタリなドライバーや、AMD/Intel 向けの Mesa です。同梱の COLMAP は、互換性のある NVIDIA のデバイスでは CUDA/Caspar を、それ以外では CPU/Ceres を使います。Brush はグラフィックスバックエンドを独自に選びます。CPU のみのソフトウェアの Vulkan は、まだエンドツーエンドで検証していません。

main ブランチの GitHub Actions の実行から `OOOSplat-0.7.1-x64-linux` の Artifact をダウンロードしたら、次のコマンドでインストールします：

```bash
sudo apt install ./OOOSplat-0.7.1-x64-linux.deb
```

`.deb` が Ubuntu のパッケージマネージャーでインストールするのは FFmpeg/FFprobe だけで、固定したバージョンの COLMAP と Brush のランタイムは同梱されています。

## インストールと使い方

サンプル素材：生成のワークフローをテストしたり試したりするには、[Quark Drive からダウンロード](https://pan.quark.cn/s/1dde892a1324)してください。

1. Windows では `OOOSplat-0.7.1-x64-windows.exe` を実行します。Apple Silicon の Mac では `OOOSplat-0.7.1-arm64-macos.dmg` を開き、OOOSplat を「アプリケーション」にドラッグします。Ubuntu 24.04 では `sudo apt install ./OOOSplat-0.7.1-x64-linux.deb` を実行します。
2. OOOSplat を起動し、上部バーにある同梱エンジンの状態が正常であることを確認します。右上の `EN / 中文` を使うと、インターフェースの言語をすぐに切り替えられます。
3. 「Task List → New Tasks」で下書きを選ぶか、Add を選んでから動画または画像のフォルダーを選びます。判別が終わるのを待ち、種類がはっきりしない場合は素材のボックスで通常かパノラマかの処理を選びます。
4. プロジェクトのルートを選びます。OOOSplat は最後に使った場所を記憶します。
5. Fast、Balanced、Detailed のいずれかの品質プリセットを選びます。
6. COLMAP の高速化を確認し、「Start Generation」を選びます。100 枚を超える画像では、ファイル名が撮影順に従っているかどうかを確認します。開始する前に回答が必要です。
7. 選んだタスクの段階、進捗、ログを左側で確認します。右側の任意のタスクをクリックすると詳細が切り替わります。生成したプロジェクトは Completed か Unfinished に表示され、完了したプロジェクトでは Preview を使えます。
8. 「Preview → Adjust」でモデルを編集します。変形、切り抜き、削除はプロジェクトに自動で保存され、各プロジェクトのビューはこのデバイスにローカルで保存されます。「Save edits」は、現在の編集とビューが保存されたことを確認するためのものです。標準 PLY、圧縮 PLY、SPZ、SOG は「Export」から直接選べ、形式をもう一度確認する手順はありません。Adjust と Animation のどちらでもモデルを書き出せます。
9. Animation に切り替えて Portrait か Landscape を選び、Export → Export video を使うと、それぞれ 1080×1920 または 1920×1080 の、30 FPS、23 秒の MP4 が書き出されます。Adjust と Animation のどちらでもオフラインの HTML を書き出せます。書き出しには現在の編集が使われ、キャンセルでき、完了すると出力先のフォルダーを開けます。
10. 細部を追加するには、完了したプロジェクトで Reshoot を選びます。新しい素材と種類を選び、必要に応じて品質と Automatic retry を調整してから、モデルの状態についての質問に答えます。通常の入力では、撮影の設定が同じかどうかも尋ねられます。別途プランを確認する手順はありません。Automatic retry を有効にすると、必要な場合に時間のかかるフォールバックを使えるようになります。新しいビューと古いビューは重なり合い、同じシーンを写している必要があります。

使い方の注意：

- パネルの間の仕切りをドラッグするとサイズを変えられます。ダブルクリックするとデフォルトの比率に戻ります。
- 右下のパーセントのボタンで、インターフェースの拡大率を縮小、リセット、拡大できます。
- 下書きはローカルに保存され、再起動後に素材が確認し直されます。実行中のタスクのパラメーターは変更できませんが、他の下書きは引き続き編集できます。2 つ目のタスクを同時に実行することはできません。
- 新しいタスクはドラッグして並べ替えられます。次のタスクの自動実行はデフォルトでオフで、準備ができていないタスクを飛ばすことはありません。画像の順序についての質問があると起動が一時停止し、キャンセルするとキューが無効になります。
- 新しい再撮影は元の品質を引き継ぎ、Automatic retry はデフォルトで有効になります。どちらも開始前に編集できます。通常のバッチは、解像度、アスペクト比、デバイス、レンズが異なっていてもかまいません。完全な ERP のバッチは、バッチ間でサイズが異なっていてもかまいません。元の素材をもう一度提出することはできません。信頼できる画像／再構成のスナップショットがない場合は開始できません。
- 元のモデルを再利用すると、編集前の生成された PLY から初期化し、古いビューと新しいビューを、ステップを短縮したり学習率を下げたりすることなく、通常の完全なレシピで学習します。元の内部パラメーターが再利用されるのは、確認を経て、単一カメラの寸法と向きの互換性チェックに通った場合だけです。これはオプティマイザーの再開ではなく、新しい領域だけが変わることを保証するものでもありません。削除や切り抜きの編集は引き継がれません。既存のタスクは保存済みの設定を維持します。[再撮影の検証ノート](docs/RESHOOT_V2.md)を参照してください。
- タスクのページには最新の 500 件のログが残り、エンジンの完全なログはプロジェクトの `logs/` ディレクトリに残ります。学習時のランタイムモニターはデフォルトで非表示で、Settings → Interface で有効にできます。
- Delete は、元素材のコピーや中間ファイルを含むプロジェクト全体をごみ箱に移します。その操作が失敗しても、OOOSplat が完全な削除にフォールバックすることはありません。

## 品質プリセット

シーケンシャルな動画と、順序が確認された画像のマッチングでは、デフォルトでプリセットのループ検出を使います。有効なループが見つかるとは限りません。再撮影では古いビューと新しいビューを別々にマッチングし、順序のある新しいビューの間ではウィンドウ 10 とプリセットのループ方針を使います。補完は Automatic retry とは独立しています。

新たに手動でサンプリングした実行では、初回の登録率が 80% を下回り、有効な初期モデルがある場合、登録されなかった区間で、狙いを絞ったブリッジの試行を 1 回行うことがあります。追加するのは実際の初期フレーム数の最大 20% までで、利用できる元のフレーム数が上限になります：初期フレームが 360 なら追加は最大 72 で、100% のサンプリングでは何も追加しません。30 フレームの最小値は引き続き適用され、それより短い素材では利用できるすべてのフレームを使います。デフォルトのサンプリングでは、下記の候補の予算を維持します。中断された古い実行は、保存済みのルールを維持します。1.2× の上限が制御するのはフレーム数であって、時間や成功率ではありません。[ブリッジのルールと検証](docs/MANUAL_BRIDGE_V3.md)を参照してください。

以前の最適化の挙動は、現在は標準になりました。Fast/Balanced は作業解像度の制限を維持し、Detailed はアップスケールせずに元の学習解像度を使います。画像では有効な入力をすべて残し、動画のサンプリングでは、元の FPS から正確に導いたデフォルトも含め、品質プリセットより上の段階的な刻みを使います。[生成のデフォルト設定](docs/GENERATION_POLICY.md)と[従来の設定ガイド](docs/pipeline-optimization-config.en.md)を参照してください。

下の表は通常の入力についてのものです。通常の素材の Detailed では、登録された学習ビューの有効なピクセル数と、再構成のトラックの平均の長さから、係数 0.5 でスプラット数の予算を決めます。完全に透明なピクセルは除外され、初期化の点が黙って取り除かれることはありません。[Detailed の予算の計算式と範囲](docs/FINE_SPLAT_BUDGET.md)を参照してください。

パノラマには専用の学習レシピと点の予算があります。Detailed はネイティブの ERP を保ち、アップスケールせずに `floor(ERP width / 4)` で各面を導きます。点の予算は元の球面のモデルを使い、6 倍にはしません。上限は、実際に達する数、品質、十分なメモリのいずれも保証しません。

| プリセット | デフォルトの初期／ブリッジ候補の目標 | 動画の作業時の長辺 | COLMAP の長辺／最大特徴点数 | Brush の学習 |
| --- | ---: | ---: | ---: | --- |
| Fast | 6 / 9 FPS | 最大 1,600 | 1,200 / 4,096 | 8,000 イテレーション、最大解像度 1,600 |
| Balanced | 8 / 12 FPS | 最大 1,920 | 1,600 / 8,192 | 15,000 イテレーション、最大解像度 1,920 |
| Detailed | 12 / 15 FPS | 元の解像度 | 3,200 / 16,384 | 30,000 イテレーション、元の学習解像度、有効ピクセル数／トラックによる予算（係数 0.5） |

短い動画では、元の素材に十分なフレームがあれば、選ぶフレームの最小値を 30 とします。そのため、初期と候補のサンプリングレートが表の目標を超えることがありますが、元の FPS やフレーム数を超えることはありません。

Detailed の学習解像度は、VRAM による段階分けを使わなくなりました。Automatic retry を有効にすると、3 つのプリセットのいずれでも、明示的な GPU の OOM のあとに、失敗したプロセスの最後の信頼できるスプラット数の 80% で、解像度を下げずに 1 回リトライすることがあります。数がわからない場合、実際には減らない場合、予算が初期化を下回る場合は、そのまま失敗します。バックエンドの互換性のためのフォールバックとフレームの補完は、それぞれ独立して有効のままです。

中断されたタスクは、固定された設定を維持します。失敗したタスクを手動でリトライすると新しい方針を使い、互換性のない作業キャッシュはアーカイブされます。元のファイル、公開済みのモデル、編集は保持されます。

## プロジェクトとファイルの場所

生成のたびに、プロジェクトのルートの下に別々のディレクトリが作られます：

```text
<projects-root>\<yyyyMMdd-HHmmss_source-name>\
  final.ply             Final Gaussian Splatting file
  project.json          Project metadata and result metrics
  edit.ply              Exported edits; safely replaced on later PLY exports (optional)
  edit.compressed.ply   Compressed PLY export (optional)
  edit.spz              SPZ v4 export (optional)
  edit.sog              Bundled SOG export (optional)
  preview.mp4           First optional animation-preview video export
  preview-2.mp4         Later video exports are automatically numbered
  preview-landscape.mp4 Landscape video (later exports are automatically numbered)
  preview.html          Offline viewer (later exports use preview-2.html, etc.)
  state.json            Pipeline state
  source\
    input.<ext>         Source-video copy (video projects)
    images\             Normalized source copies (image-sequence projects)
  work\
    frames\             Extracted or prepared input images
    colmap_frames\      Dedicated COLMAP images, byte-identical to prepared input
    masks\              COLMAP masks for transparent input (when needed)
    reshoot-masks\      COLMAP masks for reshoot input (when needed)
    colmap\             COLMAP database and sparse reconstruction
    brush\              Brush dataset and training intermediates
  logs\                 Complete FFmpeg, COLMAP, and Brush engine logs
```

プロジェクト名からは、Windows で使えない文字が取り除かれます。名前が重複した場合は、`-2` や `-3` のような接尾辞が付きます。

アプリケーションの設定とプロジェクトのインデックスは、次の場所に保存されます：

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
%LOCALAPPDATA%\SplatStudio\tasks.json
%LOCALAPPDATA%\SplatStudio\telemetry.json
```

これらは Windows のパスです。macOS と Linux では、それぞれのローカルのアプリケーションデータのディレクトリを使います。開始していない下書き、UI の設定、プレビューのビューはローカルの WebView のストレージに保存され、プロジェクトのファイルと一緒には移動しません。

## 匿名の利用統計

OOOSplat は、安定性と段階ごとの所要時間を把握するため、デフォルトで匿名の利用統計を収集します。右上の **Settings -> Privacy** でいつでもオフにでき、オフにすると匿名の分析のリクエストは送られません。

送信される内容：

| フィールド | 説明 |
| --- | --- |
| インストール ID | 初回起動時に生成されるランダムな UUID。ハードウェアのシリアル番号、MAC アドレス、デバイスのフィンガープリントは読み取りません |
| アプリのバージョン、OS、CPU のアーキテクチャ | たとえば `0.7.1` / `windows` / `x86_64` |
| イベント名 | `daily_active`、`generation_started`、`generation_completed`、`generation_failed`、`pipeline_stage_completed`、`planner_evaluation`。`daily_active` は 1 日に最大 1 回送られ、アプリのバージョンが変わった日にはもう 1 回送られます |
| 品質プリセットと入力の種類 | `balanced` / `video` のような列挙値。画像シーケンスの入力では `images` を報告します |
| 段階ごとと全体の所要時間 | ミリ秒 |
| 基本的な生成イベントでのフレーム数と動画の長さ | 区間に丸めた値。Planner の評価には、実際の素材と再構成の数も含まれます |
| 自動最適化の効果の指標 | 匿名の素材の寸法と数、プラン、登録された画像と 3D 点、ブリッジによる回復、Brush の設定、Splat の数、PLY のサイズ、段階ごとの所要時間。生成、再開、再撮影のいずれでも記録し、既存のフィールドの意味を保ちます。Planner の状態の代わりにリトライの同意を使うことはありません |
| Planner の GPU の情報 | ベンダーの列挙値と検出した VRAM の合計。アダプターの完全な名前やデバイスのシリアル番号は含みません。取得できない値は空または不明のままです |
| 失敗した段階とエラーコード | `colmap_mapper_failed` のような列挙値。生のエラーのテキストは含みません |

匿名の分析で送られることのないもの：動画、画像、PLY ファイルを含む元の素材、ファイル名、パス、プロジェクト名、ログとコマンドの出力、ユーザー名やあらゆる個人情報。

### 任意のエラーレポート

生成が失敗したあとで **Send error report** を選び、伏せ字処理済みのレポート全体を確認してから **Agree and send** を選びます。レポートには、失敗の理由、エラーの詳細、失敗時点の最新のログの抜粋（最大 200 行 / 64 KiB）、アプリのバージョン、システムのバージョン、検出できるすべてのグラフィックカードが含まれます。素材、モデル、デバイスのシリアル番号、匿名のインストールの識別子は含みません。パス、ファイル名、ユーザー名は取り除かれます。取得できないハードウェアのフィールドは空のままで、実際の GPU を推測することはありません。

同意は別個の 1 回限りのものです：匿名の分析を無効にしたままでも、それを有効にすることなくレポートを送れます。プレビューをキャンセルすると何もアップロードされません。失敗した場合は手動でのみリトライでき、送信に成功するとレポートの ID が表示されます。ローカルのレポートとプレビューは、確認の時間制限なしに現在のアプリのセッションの間は有効で、アプリを閉じると消去されます。サーバーが受け取ったレポートは 30 日間保持されます。受信側のサービスがデプロイされていない場合やネットワークが使えない場合、アプリは成功したと主張するのではなく、送信の失敗を報告します。[受信サービスの統合ガイド](server/diagnostics/README.md)を参照してください。

## 同梱エンジン

| エンジン | 固定したバージョン／ビルド | 用途 |
| --- | --- | --- |
| FFmpeg / FFprobe | Windows x64 は固定した 8.1 系の LGPL の共有ライブラリ版の月末ビルド（n8.1.3-9-g29e619e767）、macOS arm64 は 8.1.2 の LGPL の共有ライブラリ版、Ubuntu はシステムのパッケージ | 動画の解析とフレームの抽出 |
| COLMAP | 4.2.1 の同じコミット。Windows/Linux は CUDA + Caspar/Ceres、macOS は Ceres CPU | 特徴点の抽出、マッチング、カメラの再構成 |
| Brush | OOOBrush ooo-v1.0.0。Windows/Linux x64、macOS arm64 | ヘッドレスの Gaussian Splatting の学習と PLY の書き出し |

Windows、Ubuntu、macOS の入手元と完全性の方針は、[`engines/manifest.json`](engines/manifest.json)、[`engines/manifest.linux.json`](engines/manifest.linux.json)、[`engines/manifest.macos.json`](engines/manifest.macos.json) に記録されています。大きなエンジンのファイルは Git にコミットされておらず、開発者は `npm run setup:engines` で復元します。リリースのビルドでは、入手元、ハッシュ、アーキテクチャ、動的ライブラリの依存関係の閉包、Brush の CLI の互換性を検証します。

サードパーティのライセンスと通知は [`licenses/`](licenses/) にあります：

- FFmpeg：Windows では LGPL-3.0-or-later、macOS では LGPL-2.1-or-later。どちらのビルドも GPL/nonfree のコンポーネントを無効にしています。
- COLMAP：BSD-3-Clause。
- Brush：Apache-2.0。

## ローカルでの開発

### COLMAP の開発用パッケージをダウンロードする

Windows と Ubuntu では、`npm run setup:engines` が COLMAP と Brush をダウンロードして検証します。Windows では固定したバージョンの FFmpeg/FFprobe も復元し、Ubuntu ではシステムの FFmpeg/FFprobe を使います。macOS では、下記の Apple Silicon の開発ワークフローでランタイムを準備します。

- COLMAP は [**ooosplat-colmap**](https://github.com/ooolabdev/ooosplat-colmap) からダウンロードされます。
- Brush は [**OOOBrush**](https://github.com/ooolabdev/OOOBrush) からダウンロードされます。

手動でインストールする場合は、各リポジトリの Releases ページから自分のプラットフォーム用のパッケージをダウンロードします。展開した COLMAP の中身を次の場所にコピーします：

- Windows：`engines/colmap/`（`bin/colmap.exe`）。
- Linux：`engines/linux/colmap/`（`bin/colmap`）。
- macOS：`engines/macos/arm64/colmap/`（`bin/colmap`）。

ランタイムのライブラリ、ライセンス、メタデータは、ラッパーのディレクトリを追加せずにまとめておいてください。`npm run dev:local` と `npm run build:local` も、固定したバージョンの OOOBrush のリリースを準備して検証します。

ディレクトリの構成と検証の詳細は、[エンジンの開発用パッケージのガイド](engines/README.md)を参照してください。

### Windows の開発環境

- Node.js 22.12 以降。
- Rust stable、ターゲットは `x86_64-pc-windows-msvc`。
- Desktop development with C++ を含む Visual Studio 2022 Build Tools。
- Tauri 2 が必要とする WebView2 の開発／実行環境。

依存関係をインストールして開発モードを起動します：

```powershell
npm install
npm run setup:engines
npm run tauri -- dev
```

### Ubuntu 24.04 Alpha の開発

Ubuntu のシステムの依存関係、Node.js、Rust をインストールしたら、次を実行します：

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

リポジトリのルートで `./scripts/start-app-linux.sh`（または `npm run start:app:linux`）を実行すると、ローカルのエンジンとライセンスの対応を検証し、ソースが変わった場合だけリリース用の実行ファイルをビルドし直して、OOOSplat を起動します。

Ubuntu のセットアップでは、検証済みの COLMAP を `engines/linux/colmap/` に、Brush を `engines/linux/brush/` に復元します。FFmpeg/FFprobe はシステムのパッケージのままです。`.deb` には COLMAP/Brush が組み込まれており、システムの COLMAP には依存しません。

Ubuntu 24.04 Alpha の CI ワークフローは `.github/workflows/ubuntu.yml` にあります。標準の GitHub のランナーで、フロントエンドのテスト／ビルド、ライセンスの対応、Rust のテスト、Clippy、FFmpeg の統合、`.deb` の作成、パッケージの検証、インストーラーと SHA-256 のアップロードをカバーしています。Brush のエンドツーエンドのカバーには、動作するグラフィックスバックエンドを持つホストかセルフホストのランナーが必要です。パイプライン全体は現在 NVIDIA で検証しています。AMD、Intel、ソフトウェアの Vulkan でのテスト結果を歓迎します。

### macOS 15+ Apple Silicon Alpha の開発

Node.js、Rust、Xcode Command Line Tools を入れた Apple Silicon の Mac で、次を実行します：

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

上記のダウンロードして起動するワークフローには、混在した macOS のランタイムがハッシュで固定されている必要があります。`engines/manifest.macos.json` にある配布物のハッシュは現在空なので、通常の `setup:engines` はインストールを拒否します。最初のローカルの開発では、[macOS の CI ワークフロー](.github/workflows/macos.yml)に従って FFmpeg/Brush のアーカイブをビルドして検証する必要があります。COLMAP は、別途固定した `ooosplat-colmap` のリリースからインストールされます。この制限はソースからの開発環境の準備に関するもので、すでにエンジンを含むインストーラーの利用には関係ありません。

ソースからのビルドに必要なものは、Homebrew が `npm run setup:build-deps:macos` で用意し、続いて `npm run build:engines:macos` を実行します。macOS の CI は、その実行のローカルのアーカイブを検証してから、署名のないアプリと DMG を作成します。マニフェストのアーカイブの通常のダウンロードを有効にする前に、アーカイブと完全性のファイルのハッシュを確認して固定してください。

### テストとチェック

```powershell
# Frontend tests
npm test

# Frontend production build and TypeScript type-check
npm run build

# Rust tests
cargo test --manifest-path src-tauri\Cargo.toml

# Rust static checks
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings

# Bundled engine version, hash, and CUDA runtime checks
npm run verify:engines

# First-party and third-party license checks
npm run verify:licenses
```

### Windows のインストーラーをビルドする

```powershell
npm run package:windows
```

NSIS のインストーラーは次の場所に書き出されます：

```text
dist-artifacts\OOOSplat-0.7.1-x64-windows.exe
```

最初のビルドの前に `npm run setup:engines` を実行してください。Tauri の `beforeBuildCommand` はエンジンのチェックとフロントエンドの本番ビルドを自動で実行しますが、パッケージングの間に暗黙にネットワークにアクセスすることはありません。

## CLI

リポジトリには、診断用の `splatstudio` CLI も用意されています：

```powershell
# Check all bundled engines
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- health

# Read video metadata
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- probe "D:\Videos\orbit.mp4"

# Show the frame extraction plan without writing frames
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- plan "D:\Videos\orbit.mp4" --quality balanced

# Extract frames only
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- extract "D:\Videos\orbit.mp4" "D:\Frames" --quality fast

# Run the complete pipeline
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Videos\orbit.mp4" --projects-root "D:\Splat Projects" --quality balanced

# Use the same command with a folder for an image sequence
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Photos\object" --projects-root "D:\Splat Projects" --quality balanced
```

開発や診断のために、グローバルな `--engine-dir <path>` 引数か `OOOSPLAT_ENGINE_DIR` 環境変数で、デフォルトのエンジンのディレクトリを上書きできます。

Linux の FFmpeg/FFprobe は、それぞれ個別の環境変数／PATH による検出を維持します。Brush のデフォルトは管理下の `linux/brush/brush_app` で、PATH へのフォールバックはありません。診断用に明示的に `OOOSPLAT_BRUSH` で上書きする場合は、新しい CLI に対応している必要があります。COLMAP は管理下のルートの下にある `linux/colmap/bin/colmap` としてのみ解決され、個別の `OOOSPLAT_COLMAP` や PATH へのフォールバックはありません。

## MCP（オプション）

Settings → MCP で有効にすると、Codex などのローカルの AI エージェントが、通常／パノラマの入力を確認し、生成と再撮影の回答を集め、タスクを開始／再開し、状態とログを読めるようになります。エージェントは、プレビューを開かずに、保存済みの編集を標準 PLY、圧縮 PLY、SPZ、SOG として書き出すこともできます。ファイルはプロジェクトの `exports/` ディレクトリにとどまり、SPZ の範囲の調整にはユーザーの承認が必要です。このサービスはデフォルトでオフで、`127.0.0.1` でのみ待ち受け、トークンは必要ありません。

接続と設定の例は [MCP のドキュメント](docs/mcp-v1.md)を参照してください。

## よくある質問

### COLMAP が GPU ではなく CPU を使っているのはなぜですか？

Windows と Ubuntu では、固定したバージョンの CUDA のランタイムが正常で、NVIDIA のドライバー 580.00 以降と Compute Capability 7.5 以上であることが確認できた場合に、OOOSplat は COLMAP の GPU による高速化を有効にします。Caspar では、さらに小さな BA のテストに成功する必要があります。失敗した場合は Ceres CPU を選びますが、それ以外の点で利用できる SIFT の GPU による高速化は無効にしません。設定のページには検出した能力が表示され、実行中のタスクの詳細とログには実際に使われているバックエンドが表示されます。バックグラウンドでの検出が一時的にタイムアウトしても、実行中のタスクが CPU に切り替わることはありません。macOS の COLMAP は CPU を使い、Brush はグラフィックスバックエンドを独自に選びます。

### OOOSplat はさまざまな GPU の組み合わせをどう扱いますか？

COLMAP と Brush は別々の段階です。COLMAP の GPU による高速化には、互換性のある NVIDIA の CUDA 環境が必要です。Brush は、Windows と Linux では Vulkan を、Apple Silicon の macOS では Metal を使います。よくある構成は次のように扱われます。プラットフォームを明記していない行は Windows についてのものです：

| プラットフォームと GPU の構成 | COLMAP | Brush の学習 |
|---|---|---|
| Windows：NVIDIA の単体 GPU が 1 つ | ドライバーと Compute Capability の要件を満たせば CUDA を使う | 最初の単体 GPU を明示的に選び、Vulkan で動かす |
| Intel の内蔵 GPU ＋ NVIDIA の単体 GPU が 1 つ | 互換性のある NVIDIA の GPU を CUDA で使う | Intel の内蔵 GPU ではなく、最初の単体 GPU を明示的に選ぶ |
| AMD の内蔵 GPU ＋ NVIDIA の単体 GPU が 1 つ | 互換性のある NVIDIA の GPU を CUDA で使う | 最初の単体 GPU を明示的に選び、Brush の子プロセスに限って AMD Switchable Graphics の暗黙のレイヤーを回避する |
| AMD のみ、または Intel の内蔵 GPU ＋ AMD の単体 GPU | CPU にフォールバックする | Vulkan が自動で AMD の GPU を選ぶ。AMD のグラフィックスのレイヤーは無効にしない |
| Intel の内蔵または単体 GPU のみ | CPU にフォールバックする | Vulkan が自動で Intel の GPU を選ぶ |
| 複数の NVIDIA の GPU | Compute Capability が最も高い互換性のある GPU を選び、同じ場合は NVIDIA のインデックスが小さいほうを選ぶ | Vulkan と CUDA のインデックスを確実に対応づけられないため、Brush のデバイスのインデックスは強制しない。Vulkan が自動で選ぶ |
| AMD の単体 GPU ＋ NVIDIA の単体 GPU | COLMAP には互換性のある NVIDIA の GPU を使う | 現在は NVIDIA が 1 つの場合の優先の経路に入るが、ベンダーをまたぐ Vulkan の順序は保証できない。Windows のグラフィックの設定で `brush_app.exe` を高パフォーマンスにし、タスクのログで実際のアダプターを確認すること |
| 古い NVIDIA のドライバー、対応していない NVIDIA のモデル、または検出の失敗 | CPU にフォールバックする | Brush は引き続き Vulkan で利用できる GPU を選ぼうとするが、NVIDIA を強制はしない |
| 使えるグラフィックスバックエンドの GPU がない | COLMAP には CPU を使う | Brush は起動しない場合がある。CPU のみのソフトウェアのグラフィックスは、エンドツーエンドで検証していない |
| macOS：Pro、Max、Ultra を含む Apple Silicon の M シリーズ | 現在は CPU を使う。CUDA は有効にならない | M シリーズの Metal の GPU とユニファイドメモリを自動で使う |
| Ubuntu Alpha：互換性のある NVIDIA の GPU | ランタイム、ドライバー、Compute Capability のチェックに通れば CUDA を、そうでなければ CPU を使う | Windows 固有のアダプターの上書きなしに、Vulkan が自動で選ぶ |
| Ubuntu Alpha：AMD、Intel、または互換性のある NVIDIA の GPU を検出できない | CPU を使う | Vulkan が利用できる GPU を自動で選ぶ |

新しい方針では、Detailed は VRAM による Low、Standard、Large の段階分けではなく、元の学習解像度を使います。通常とパノラマの予算は、上記の品質プリセットにあるそれぞれのルールに従います。学習を開始できても、あらゆる解像度や点数に十分なリソースがあるとは限りません。

アダプターの強制選択と AMD の暗黙のレイヤーの回避策は、個々の `brush_app.exe` の子プロセスにだけ渡されます。OOOSplat は、システムの環境、ドライバーの設定、他のアプリケーションを変更せず、Brush を D3D12 に切り替えることもしません。AMD のみのシステムには、AMD の暗黙のレイヤーの回避策は適用されません。

Brush のログに `Device Lost` と出た場合、OOOSplat はそれを GPU の OOM と区別し、OOM のリトライは行いません。コンピューターを電源につなぎ、GPU を多く使う他のアプリケーションを閉じ、Windows の設定 → システム → ディスプレイ → グラフィックで `brush_app.exe` を高パフォーマンスに設定してください。点の上限を下げた 1 回のリトライには、上記のとおり、明示的な GPU の OOM、Automatic retry の有効化、信頼できる数が必要です。

M シリーズの CPU と GPU はユニファイドメモリを共有しますが、新しい方針では、それを理由に Detailed を Low に格下げすることはありません。ネイティブの解像度での学習は、それでも利用できるリソースを超えることがあります。新しい方針での macOS と Linux のエンドツーエンドのハードウェアのカバー範囲については、検証の記録を参照してください。

### OOOSplat が低い登録率について警告するのはなぜですか？

登録率が低いのは、たいてい COLMAP が連続して重なり合う視点を十分に見つけられなかったことを意味します。タスクは引き続き Brush に進みますが、結果の品質に影響することがあります。露出が安定し、フレームが鮮明で、動きが連続していて、視点が十分に重なる周回の動画を使ってください。速い回転、強い反射、単色の広い領域、動く被写体は避けてください。

### プロジェクトがディスク容量を多く使うのはなぜですか？

各プロジェクトは、診断と再開のために、元素材のコピー、入力画像、COLMAP のデータ、Brush の中間ファイルを保持しています。画像シーケンスでは、物理的なコピーの重複を減らすために内部のハードリンクを優先して使います。結果を確認したら、Task List の Delete を使ってプロジェクト全体をごみ箱に移してください。

### final.ply を OOOSplat で直接表示できますか？

はい。タスク一覧で完了したプロジェクトの「Preview」を選ぶと、その `.ply` が専用のプレビューのワークスペースで開きます。「Adjust」では、モデル全体の Transform と、矩形、球、ボックスによる Gaussian の選択ができ、非破壊の削除、切り抜き、反転、元に戻す、やり直しに対応しています。編集はプロジェクトに自動で保存されます。「Save edits」は、モデルのファイルを作らずに、現在の編集とビューが保存されたことを確認するためのものです。「Export」からモデルの形式を直接選ぶと、編集したモデルを作成するか安全に置き換え、元の `final.ply` は変わりません。開き直すと、このデバイスに保存された編集とビューが復元されます。黄色の選択と元に戻す履歴は、セッションをまたいで残りません。「Animation」では、5 秒の表示演出、8 秒の衝撃波、連続した周回、透かし入りの縦長／横長の MP4 の書き出し、オフラインの HTML の書き出しが加わります。外部の標準／圧縮 PLY、SOG、SPZ、SPLAT、KSPLAT のファイルは「Completed → Import」から取り込めます。LCC/LCC2、Streamed SOG、2DGS、通常の点群の PLY ファイルには対応していません。

`WASD` で水平面上を移動し、デフォルトでは 1 秒あたり現在の表示の高さの 15% の速さで進みます。右下のスライダーで 0.1×〜5.0× の倍率をかけられ、`Shift` でさらに 3 倍の速さになります。Auto locate view は、どのビューからでも最初の学習カメラに戻ります。パノラマのプロジェクトでは、最も早く登録されたパノラマの正面を使います。信頼できる学習カメラがない場合は無効になります。プレビューを開いたときは、引き続き保存済みのビューが最初に復元されます。[ナビゲーションのノート](docs/PREVIEW_NAVIGATION.md)を参照してください。

### 非常に大きな PLY ファイルをプレビューできますか？

256 MiB を超える PLY ファイルは、転送中にファイル全体をバッファーすることを減らすため、約 1 MB ずつの Range リクエストで読み込みます。これによって SH のデータが圧縮、ダウンサンプリング、破棄されることはありません。解析、並べ替え、GPU のテクスチャには引き続き RAM と VRAM が必要です。モデルを開けるかどうかはそのサイズとデバイスによるので、大きなモデルがすべて読み込めるとは限りません。

### どのモデルの形式で書き出せますか？

プレビューの Export メニューは、モデル、縦長／横長の動画、単一ファイルのオフライン HTML に対応しています。モデルの選択肢：

| 形式 | 用途とサイズ |
| --- | --- |
| 標準 PLY | 完全な精度で、たいてい最も大きい。アーカイブや編集の継続向け |
| 圧縮 PLY | 小さく、非可逆。互換性のあるビューアーが必要 |
| SPZ v4 | 非常に小さく、非可逆。共有向け |
| バンドル版の SOG | たいてい圧縮率がより高く、非可逆。Web での表示向け |

SOG と SPZ はたいてい小さくなりますが、実際のサイズはさまざまで、圧縮によって細部に影響が出ることがあります。処理は形式を選んでから始まり、中間の PLY は作りません。元の `final.ply` は変わらず、失敗やキャンセルの場合も既存の書き出しは保持されます。圧縮は推定メモリ量のしきい値で止められなくなりましたが、大きなモデルではメモリが足りなくなれば失敗することがあります。SPZ の座標がその範囲を超える場合は、書き出し用のコピーだけを中央に寄せ、必要なら均一に縮小するために、明示的な確認が必要です。位置、寸法、単位が変わることがあり、安全にエンコードできない場合は書き出しを止めます。[書き出しのノートと検証の記録](docs/model-export-verification.md)を参照してください。

オフラインの HTML は編集を保持し、表示とアニメーションの再生に対応していますが、編集には対応していません。大きなモデルは、引き続きデバイスの RAM と VRAM に制限されます。

## 技術

- デスクトップのフレームワーク：Tauri 2
- バックエンド：Rust と Tokio
- フロントエンド：React 19、TypeScript、Vite、Zustand
- Gaussian のプレビューとアニメーション：PlayCanvas Engine と PlayCanvas React
- 動画のエンコードと多重化：WebCodecs と Mediabunny
- ネイティブのパイプライン：FFmpeg / FFprobe、COLMAP、Brush
- プロセスツリーの管理：Windows は Job Object、Linux/macOS は Unix のプロセスグループ

## 🤝 コントリビューション

コントリビューションを歓迎します！

バグの修正、Linux/macOS への対応、UI の改善、
ドキュメント、Gaussian Splatting の新機能など、どんなことでもぜひ力を貸してください。

始めるには [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。

## ライセンス

### コード

OOOSplat のファーストパーティのコードと付属のドキュメントは、[Apache License 2.0](LICENSE) のもとで公開されています。著作権の情報は [NOTICE](NOTICE) を参照してください。

### サードパーティのコンポーネント

FFmpeg / FFprobe、COLMAP、Brush、PlayCanvas、Mediabunny は、引き続きそれぞれのライセンスに従い、OOOSplat と一緒に配布されるからといって Apache-2.0 のソフトウェアになるわけではありません。直接のコンポーネント、バージョン、入手元、ライセンスのファイルについては、[サードパーティの通知](licenses/THIRD_PARTY_NOTICES.txt)と[エンジンのマニフェスト](engines/manifest.json)を参照してください。この一覧は、Qt、Boost、Ceres などの推移的な依存関係を完全に監査したものとして示しているわけではありません。

### ブランド

Apache-2.0 は、「OOOSplat」という名前、ロゴ、アイコン、その他の視覚的な識別子を商標として使う許可を与えるものではありません。事実に即した言及、チュートリアル、スクリーンショット、改変していない配布、改変版の命名のルールについては、[OOOSplat の商標ポリシー](TRADEMARK_POLICY.md)を参照してください。

### 生成されたモデル

`final.ply` やその他の生成された出力は、OOOSplat を使ったというだけで、Apache、GPL、LGPL、その他の同梱のソフトウェアのライセンスに自動的に従うことにはなりません。この記述は、著作権の帰属を決めたり、入力やサードパーティの素材についての権利を与えたりするものではありません。完全な境界については[生成された出力](GENERATED_OUTPUTS.md)を参照してください。
