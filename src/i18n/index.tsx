import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type Locale = "zh-CN" | "en";
export type TranslationParams = Record<string, string | number>;

const zhCN = {
  "common.close": "关闭",
  "common.cancel": "取消",
  "common.retry": "重试",
  "common.saved": "已保存",
  "common.saving": "保存中",
  "common.failed": "失败",
  "common.ready": "就绪",
  "common.unavailable": "不可用",
  "common.frames": "{count} 帧",
  "common.images": "{count} 张",
  "language.switchTo": "中英文切换 / Switch language",
  "language.target": "EN",
  "top.settings": "设置",
  "top.checkingEngines": "正在检查内置引擎",
  "top.engineIssues": "{count} 个引擎异常",
  "top.enginesReady": "FFmpeg · COLMAP · Brush 就绪",
  "task.create": "01 创建新任务",
  "task.running": "运行中",
  "task.idle": "待命",
  "task.console": "生成控制台",
  "input.label": "输入素材",
  "input.typeAria": "选择输入素材类型",
  "input.video": "视频",
  "input.images": "图片",
  "input.videoTypes": "MP4 或 MOV",
  "input.imageTypes": "JPG、JPEG 或 PNG 文件夹",
  "input.selectImages": "选择图片序列文件夹",
  "input.selectVideo": "选择 MP4 或 MOV 视频",
  "input.selectImagesHint": "点击选择包含 JPG、JPEG 或 PNG 的文件夹",
  "input.selectVideoHint": "点击选择本机视频文件",
  "project.root": "项目根目录",
  "project.readingRoot": "正在读取默认目录",
  "project.rootHint": "每次生成会在此处创建独立项目文件夹，final.ply 直接保存在项目根部。",
  "quality.label": "生成质量",
  "quality.fast": "快速",
  "quality.fastHint": "快速验证素材与拍摄路径",
  "quality.balanced": "均衡",
  "quality.balancedHint": "质量与处理时间的推荐平衡",
  "quality.high": "精细",
  "quality.highHint": "更充分地利用视频画面细节",
  "gpu.detecting": "正在检测 COLMAP GPU 加速…",
  "gpu.enabled": "COLMAP GPU 加速已开启",
  "gpu.cpu": "COLMAP 使用 CPU",
  "gpu.reading": "正在读取 COLMAP 加速能力",
  "gpu.memory": "{value} GB 显存",
  "gpu.driver": "驱动 {value}",
  "gpu.requirements": "最低要求：驱动 {driver}，Compute Capability {capability}",
  "metrics.imageCount": "图片数量",
  "metrics.duration": "素材时长",
  "metrics.resolution": "分辨率",
  "metrics.processingImages": "处理图片",
  "metrics.estimatedFrames": "预计帧数",
  "metrics.keepAll": "全部保留",
  "metrics.approx": "约 {value}",
  "metrics.estimate": "预计时长",
  "metrics.analyzing": "分析中",
  "alpha.imagesTitle": "检测到透明图片",
  "alpha.videoTitle": "检测到 Alpha 通道",
  "alpha.imagesHint": "将保留 PNG Alpha 并自动生成 COLMAP Mask",
  "alpha.videoHint": "将自动提取透明画面和 COLMAP Mask · {format}",
  "sequence.title": "大型图片序列",
  "sequence.hint": "超过 500 张图片，穷举匹配可能需要较长时间和更多磁盘空间；开始生成前会再次确认。",
  "generate.analyzing": "正在分析素材",
  "generate.start": "开始生成",
  "progress.title": "实时进程",
  "progress.preparing": "正在准备任务",
  "progress.stage": "当前阶段",
  "progress.progress": "进度",
  "progress.elapsed": "总耗时",
  "progress.estimated": "估算 {value}%",
  "progress.continuing": "持续运行",
  "progress.running": "运行中",
  "progress.log": "任务日志",
  "progress.logCount": "最近 {count} / 500 条",
  "progress.terminating": "正在终止任务",
  "progress.cancel": "取消任务并终止所有进程",
  "progress.cancelError": "无法终止任务：{detail}",
  "progress.privacyError": "无法保存隐私设置：{detail}",
  "stage.material": "素材分析",
  "stage.frames": "画面准备",
  "stage.features": "特征提取",
  "stage.matching": "图像匹配",
  "stage.reconstruction": "相机重建",
  "stage.training": "Splat 训练",
  "stage.export": "结果发布",
  "stage.completed": "已完成",
  "stage.failed": "任务失败",
  "stage.cancelled": "已取消",
  "stage.preparing": "准备",
  "history.title": "02 历史任务",
  "history.aria": "项目成果",
  "history.refresh": "刷新",
  "history.completed": "已完成",
  "history.unfinished": "未完成",
  "history.projects": "{count} 个项目",
  "history.emptyTitle": "还没有生成项目",
  "history.emptyHint": "选择视频或图片和项目目录后开始生成，成果会自动出现在这里。",
  "project.date": "生成日期",
  "project.elapsed": "耗时",
  "project.quality": "档位",
  "project.opening": "正在打开",
  "project.preview": "预览",
  "project.resume": "继续任务",
  "project.reveal": "在文件管理器中显示",
  "project.delete": "删除",
  "status.running": "处理中",
  "status.completed": "已完成",
  "status.failed": "失败",
  "status.cancelled": "已取消",
  "status.interrupted": "已中断",
  "layout.resize": "调整创建任务与历史任务面板宽度",
  "zoom.aria": "界面缩放",
  "zoom.out": "缩小界面",
  "zoom.in": "放大界面",
  "zoom.resetTitle": "恢复 100%",
  "zoom.reset": "恢复",
  "cancel.title": "正在终止任务",
  "cancel.description": "正在关闭当前阶段及其子进程，请稍候。完成后此窗口会自动关闭。",
  "preview.preparingModule": "正在准备预览模块",
  "error.generic": "处理失败，请查看项目日志。",
  "error.cancelledNeedle": "取消",
  "privacy.firstUse": "首次使用设置",
  "privacy.settingsPath": "设置 / 隐私",
  "privacy.improve": "帮助改进 OOOSplat",
  "privacy.title": "隐私",
  "privacy.closeAria": "关闭隐私设置",
  "privacy.intro": "分享匿名的使用与性能统计，帮助我们改善 OOOSplat 的稳定性和处理体验。是否参与完全由你决定，之后也可以随时关闭。",
  "privacy.mayCollect": "可能收集",
  "privacy.collect1": "应用版本、操作系统和 CPU 架构",
  "privacy.collect2": "随机匿名安装 ID",
  "privacy.collect3": "生成成功或失败及安全错误码",
  "privacy.collect4": "质量档位和流水线阶段耗时",
  "privacy.neverCollect": "绝不收集",
  "privacy.never1": "视频、图片或高斯泼溅文件",
  "privacy.never2": "文件名、路径和项目名称",
  "privacy.never3": "项目内容、日志和命令输出",
  "privacy.never4": "用户名或任何个人信息",
  "privacy.uuid": "每台设备只生成一个完全随机的 UUID，不读取硬件序列号、MAC 地址或设备指纹。",
  "privacy.noEndpoint": "当前构建尚未配置统计接收端点，因此不会产生遥测网络请求。",
  "privacy.debug": "当前为遥测调试模式：仅在本机输出脱敏 JSON，不发送网络请求。",
  "privacy.decline": "不用了",
  "privacy.saving": "正在保存…",
  "privacy.share": "分享匿名统计",
  "privacy.analytics": "匿名使用统计",
  "privacy.analyticsHint": "分享匿名的使用与性能数据，帮助改进 OOOSplat。不会收集任何素材、文件路径或个人信息。",
  "privacy.collectSummary": "收集：",
  "privacy.collectSummaryText": "版本、系统、架构、匿名安装 ID、生成结果和阶段耗时。",
  "privacy.noCollectSummary": "不收集：",
  "privacy.noCollectSummaryText": "视频、图片、高斯文件、文件名、路径、项目内容和个人信息。",
  "privacy.network": "网络状态：",
  "privacy.networkOff": "此构建未配置统计端点，不会发送请求。",
  "privacy.networkDebug": "调试模式，仅输出脱敏 JSON。",
  "dialog.largeSequenceTitle": "大型图片序列",
  "dialog.selectVideoTitle": "选择视频文件",
  "dialog.selectImagesTitle": "选择图片序列文件夹",
  "dialog.selectProjectRootTitle": "选择项目根目录",
  "dialog.savePlyTitle": "导出 Gaussian PLY",
  "dialog.largeSequence": "该文件夹包含 {count} 张图片。穷举匹配的计算量和数据库占用会随图片数量平方增长，处理可能需要很长时间。\n\n仍要继续生成吗？",
  "dialog.continue": "继续生成",
  "dialog.deleteTitle": "删除项目",
  "dialog.deleteProject": "将“{name}”及其中的源素材、输入画面、COLMAP、Brush 和日志全部移入回收站。\n\n此操作无法在应用内撤销。",
  "dialog.trash": "移入回收站",
  "dialog.editRecoveryTitle": "编辑数据需要恢复",
  "dialog.editRecovery": "{detail}\n\n可以清除损坏的裁切与删除记录后重新打开。原始 final.ply 不会受到影响。",
  "dialog.clearEdits": "清除编辑记录",
  "viewer.aria": "高斯泼溅预览",
  "viewer.back": "返回任务",
  "viewer.title": "03 预览",
  "viewer.modeAria": "预览工作模式",
  "viewer.adjust": "调整",
  "viewer.animation": "动画",
  "viewer.adjustHint": "调整高斯泼溅的原点、大小与位置等",
  "viewer.animationHint": "调整画面并导出展示视频",
  "viewer.inputAria": "视图鼠标操作",
  "viewer.left": "左键",
  "viewer.middle": "中键",
  "viewer.right": "右键",
  "viewer.wheel": "滚轮",
  "viewer.rotate": "旋转",
  "viewer.drag": "拖动",
  "viewer.zoom": "缩放",
  "viewer.select": "框选",
  "viewer.add": "添加",
  "viewer.remove": "移除",
  "viewer.delete": "删除",
  "viewer.cancelSelection": "取消选择",
  "viewer.toolsAria": "Gaussian 编辑工具",
  "viewer.transform": "变换",
  "viewer.rectangle": "矩形选择",
  "viewer.sphere": "球选择",
  "viewer.box": "盒选择",
  "viewer.viewsAria": "正交视图",
  "viewer.side": "侧视",
  "viewer.front": "正视",
  "viewer.top": "顶视",
  "viewer.switchView": "切换到{view}图",
  "viewer.undo": "撤销",
  "viewer.undoTitle": "撤销（Ctrl+Z）",
  "viewer.redo": "重做",
  "viewer.redoTitle": "重做（Ctrl+Shift+Z / Ctrl+Y）",
  "viewer.deleteSelected": "删除选中",
  "viewer.resetAll": "全部撤销",
  "viewer.resetAllTitle": "恢复为原始 final.ply",
  "viewer.save": "保存",
  "viewer.saveProgress": "保存中 {value}%",
  "viewer.replay": "重新播放",
  "viewer.cancelExport": "取消导出",
  "viewer.exportVideo": "导出竖屏视频",
  "viewer.preparingExport": "准备导出",
  "viewer.rendering": "渲染 {current} / {total}",
  "viewer.packaging": "正在封装 MP4",
  "viewer.saving": "正在保存",
  "viewer.resourceNote": "预览与生成任务正在同时使用图形资源，显存不足时交互可能暂时变慢。",
  "viewer.initializing": "正在初始化 WebGL2 渲染器",
  "viewer.mounting": "正在创建高斯泼溅 GPU 资源",
  "viewer.loading": "正在读取高斯泼溅文件",
  "viewer.phaseInitializing": "初始化中",
  "viewer.phaseLoading": "加载中",
  "viewer.phaseMounting": "准备中",
  "viewer.phaseReady": "就绪",
  "viewer.phaseError": "异常",
  "viewer.unavailable": "预览不可用",
  "viewer.reload": "重新加载",
  "viewer.freezing": "正在固定裁切结果",
  "viewer.wait": "请稍候",
  "viewer.freezeFailed": "无法固定裁切结果",
  "viewer.backToEdit": "返回编辑",
  "viewer.splats": "泼溅数量",
  "viewer.selected": "已选择",
  "viewer.deleted": "已删除",
  "viewer.fileSize": "文件大小",
  "viewer.position": "位置",
  "viewer.rotation": "旋转",
  "viewer.scale": "缩放",
  "viewer.timeline": "时间线",
  "viewer.timelineValue": "显现 5s · 冲击波 8s · 环绕 24s / 圈",
  "viewer.renderer": "渲染器",
  "viewer.status": "状态",
  "viewer.project": "项目",
  "viewer.saveFailed": "保存失败",
  "viewer.unsaved": "未保存",
  "viewer.exported": "已导出",
  "viewer.videoEncoding": "视频编码",
  "viewer.checking": "检测中",
  "viewer.h264Ready": "H.264 可用",
  "viewer.retrySave": "重试保存",
  "viewer.exportFailed": "视频导出失败：{detail}",
  "viewer.saveQuestion": "保存编辑结果？",
  "viewer.saveDescription": "当前调整尚未保存为 edit.ply。项目中的裁切和删除记录会继续保留，原始 final.ply 不会被修改。",
  "viewer.skipSave": "暂不保存",
  "viewer.saveContinue": "保存并继续",
  "viewer.keepOpen": "请保持窗口开启",
  "viewer.framesProgress": "{percent}% · {current} / {total} 帧",
  "viewer.orbitAxis": "旋转轴 Y · 24 秒/圈",
  "viewer.stateSaved": "已保存",
  "viewer.stateSaving": "保存中",
  "viewer.stateFailed": "保存失败",
  "viewer.stateDirty": "未保存",
  "viewer.contextLost": "WebGL2 图形上下文已丢失。大模型可能超过当前显卡或驱动可分配的单次图形资源，请关闭其他图形应用后重新加载。",
  "viewer.invalidBounds": "PLY 已加载，但无法读取有效的模型边界。请确认该文件是 OOOSplat 生成的 Brush Gaussian PLY。",
  "viewer.initTimeout": "WebGL2 渲染器初始化超时。请更新显卡驱动和 Microsoft Edge WebView2 Runtime 后重试。",
  "viewer.cameraNotReady": "预览相机尚未就绪。",
  "viewer.selectorNotReady": "Gaussian 选择器尚未就绪",
  "viewer.textureCapacity": "当前显卡支持的最大纹理尺寸为 {maximum}，但该模型需要至少 {required}。无法安全创建预览资源。",
  "video.insecure": "当前 WebView 不是安全上下文，无法使用 H.264 视频编码器。",
  "video.noWebCodecs": "当前系统的 WebView 不支持 WebCodecs VideoEncoder。",
  "video.noAvc": "当前系统没有可用的 AVC / H.264 编码器。",
  "video.capabilityError": "无法检测 H.264 编码能力：{detail}",
  "video.invalidCanvas": "视频合成画布必须为 1080 × 1920。",
  "video.noCanvas": "无法创建视频合成画布。",
  "video.empty": "H.264 编码器返回了空文件。",
  "video.cancelled": "视频导出已取消。",
  "video.gsplatUnavailable": "PlayCanvas GSplat 系统不可用。",
  "video.sortTimeout": "等待高斯泼溅排序完成超时。请降低系统负载后重试。",
  "video.exportBusy": "已有视频正在导出。",
  "video.textureCapacity": "显卡最大纹理尺寸 {maximum} 无法导出 1080 × 1920 视频。",
  "viewer.contextReload": "图形上下文已丢失，请重新加载预览。",
  "viewer.maskRead": "删除位图读取失败（HTTP {status}）",
  "viewer.maskLength": "编辑位图长度与后端预期不一致",
  "viewer.editsUnsaved": "编辑状态尚未保存，请重试后再导出",
  "viewer.captureUnavailable": "无法读取视频取景框，请退出预览后重试。",
  "viewer.watermarkLoad": "无法加载 OOOSplat 水印 Logo。",
  "viewer.cameraUnavailable": "预览相机组件不可用",
  "viewer.captureSize": "视频取景框尺寸无效。请调整窗口大小后重试。",
  "viewer.captureOutside": "视频取景框不在渲染画面内。请调整窗口大小后重试。",
  "viewer.captureFov": "无法计算视频取景框的相机视野。",
  "viewer.framePixels": "视频帧像素数据不完整。",
  "viewer.cropChanged": "裁切区域已发生变化，请重试",
  "viewer.freezeMaskLength": "冻结裁切位图长度不一致",
  "viewer.deletedMaskCount": "删除位图与当前 Gaussian 数量不一致",
  "viewer.selectionMaskLength": "选择位图长度不一致",
  "viewer.deletedMaskLength": "删除位图长度不一致",
  "viewer.editTextures": "无法创建 Gaussian 编辑状态纹理",
  "viewer.editMaskLength": "Gaussian 编辑位图长度不一致",
  "viewer.selectorDestroyed": "Gaussian 选择器已销毁",
  "viewer.cropExpired": "Gaussian 裁切结果已过期",
  "panel.dragHint": "拖动轴标签快速调整",
  "panel.scrubTitle": "{name}：按住鼠标左键左右拖动调整，按 Shift 精细调整",
  "panel.scrubAria": "{name}拖动调整",
  "panel.modelTransform": "模型变换",
  "panel.position": "位置",
  "panel.rotation": "旋转",
  "panel.angle": "角度",
  "panel.scale": "缩放",
  "panel.uniform": "等比",
  "panel.uniformScale": "等比缩放",
  "panel.region": "选择区域",
  "panel.sphere": "球形",
  "panel.box": "盒形",
  "panel.keepInside": "{kind} · 区域内保留",
  "panel.regionPosition": "区域位置 {axis}",
  "panel.boxSize": "盒形尺寸 {axis}",
  "panel.radius": "半径",
  "panel.size": "尺寸",
  "panel.sphereRadius": "球形半径",
  "panel.noCrop": "当前未应用裁切。重新启用后，将按完整模型范围创建新的{kind}区域。",
  "panel.enableCrop": "启用{kind}裁切",
  "animation.reveal": "显现",
  "animation.shockwave": "冲击波",
  "animation.orbit": "环绕",
  "animation.complete": "完成",
} as const;

export type TranslationKey = keyof typeof zhCN;

const en: Record<TranslationKey, string> = {
  "common.close": "Close", "common.cancel": "Cancel", "common.retry": "Retry", "common.saved": "Saved", "common.saving": "Saving", "common.failed": "Failed", "common.ready": "Ready", "common.unavailable": "Unavailable", "common.frames": "{count} frames", "common.images": "{count} images",
  "language.switchTo": "中英文切换 / Switch language", "language.target": "中文", "top.settings": "Settings", "top.checkingEngines": "Checking bundled engines", "top.engineIssues": "{count} engine issues", "top.enginesReady": "FFmpeg · COLMAP · Brush ready",
  "task.create": "01 Create New Task", "task.running": "Running", "task.idle": "Standby", "task.console": "Generation console",
  "input.label": "Input media", "input.typeAria": "Choose input media type", "input.video": "Video", "input.images": "Images", "input.videoTypes": "MP4 or MOV", "input.imageTypes": "JPG, JPEG, or PNG folder", "input.selectImages": "Choose image sequence folder", "input.selectVideo": "Choose an MP4 or MOV video", "input.selectImagesHint": "Choose a folder containing JPG, JPEG, or PNG images", "input.selectVideoHint": "Choose a video file on this computer",
  "project.root": "Projects root", "project.readingRoot": "Reading default folder", "project.rootHint": "Each generation creates a separate project folder here, with final.ply saved at its root.",
  "quality.label": "Generation quality", "quality.fast": "Fast", "quality.fastHint": "Quickly validate the media and capture path", "quality.balanced": "Balanced", "quality.balancedHint": "Recommended balance of quality and processing time", "quality.high": "Detailed", "quality.highHint": "Use more of the source image detail",
  "gpu.detecting": "Detecting COLMAP GPU acceleration…", "gpu.enabled": "COLMAP GPU acceleration enabled", "gpu.cpu": "COLMAP is using the CPU", "gpu.reading": "Reading COLMAP acceleration capabilities", "gpu.memory": "{value} GB VRAM", "gpu.driver": "Driver {value}", "gpu.requirements": "Minimum: driver {driver}, Compute Capability {capability}",
  "metrics.imageCount": "Image count", "metrics.duration": "Media duration", "metrics.resolution": "Resolution", "metrics.processingImages": "Images processed", "metrics.estimatedFrames": "Estimated frames", "metrics.keepAll": "Keep all", "metrics.approx": "About {value}", "metrics.estimate": "Estimated duration", "metrics.analyzing": "Analyzing",
  "alpha.imagesTitle": "Transparent images detected", "alpha.videoTitle": "Alpha channel detected", "alpha.imagesHint": "PNG alpha will be preserved and COLMAP masks generated automatically", "alpha.videoHint": "Transparent frames and COLMAP masks will be extracted automatically · {format}",
  "sequence.title": "Large image sequence", "sequence.hint": "More than 500 images can require much more time and disk space for exhaustive matching. OOOSplat will ask again before generation starts.",
  "generate.analyzing": "Analyzing media", "generate.start": "Start Generation", "progress.title": "Live progress", "progress.preparing": "Preparing task", "progress.stage": "Current stage", "progress.progress": "Progress", "progress.elapsed": "Total elapsed", "progress.estimated": "Estimated {value}%", "progress.continuing": "Running", "progress.running": "Running", "progress.log": "Task log", "progress.logCount": "Latest {count} / 500 entries", "progress.terminating": "Stopping task", "progress.cancel": "Cancel task and stop all processes", "progress.cancelError": "Could not stop the task: {detail}", "progress.privacyError": "Could not save privacy settings: {detail}",
  "stage.material": "Media analysis", "stage.frames": "Frame preparation", "stage.features": "Feature extraction", "stage.matching": "Image matching", "stage.reconstruction": "Camera reconstruction", "stage.training": "Splat training", "stage.export": "Result publishing", "stage.completed": "Completed", "stage.failed": "Task failed", "stage.cancelled": "Cancelled", "stage.preparing": "Preparing",
  "history.title": "02 Task History", "history.aria": "Project results", "history.refresh": "Refresh", "history.completed": "Completed", "history.unfinished": "Unfinished", "history.projects": "{count} projects", "history.emptyTitle": "No projects yet", "history.emptyHint": "Choose a video or image sequence and a project folder to start. Results will appear here automatically.",
  "project.date": "Created", "project.elapsed": "Elapsed", "project.quality": "Preset", "project.opening": "Opening", "project.preview": "Preview", "project.resume": "Resume task", "project.reveal": "Show in file manager", "project.delete": "Delete",
  "status.running": "Processing", "status.completed": "Completed", "status.failed": "Failed", "status.cancelled": "Cancelled", "status.interrupted": "Interrupted",
  "layout.resize": "Resize the new-task and task-history panels", "zoom.aria": "Interface zoom", "zoom.out": "Zoom interface out", "zoom.in": "Zoom interface in", "zoom.resetTitle": "Restore 100%", "zoom.reset": "Reset", "cancel.title": "Stopping task", "cancel.description": "Closing the current stage and its child processes. This window will close automatically when finished.", "preview.preparingModule": "Preparing preview module", "error.generic": "Processing failed. Check the project logs for details.", "error.cancelledNeedle": "cancel",
  "privacy.firstUse": "First-use settings", "privacy.settingsPath": "Settings / Privacy", "privacy.improve": "Help improve OOOSplat", "privacy.title": "Privacy", "privacy.closeAria": "Close privacy settings", "privacy.intro": "Share anonymous usage and performance statistics to help improve OOOSplat stability and processing. Participation is entirely optional and can be disabled at any time.", "privacy.mayCollect": "May collect", "privacy.collect1": "App version, operating system, and CPU architecture", "privacy.collect2": "Random anonymous install ID", "privacy.collect3": "Generation success or failure and safe error codes", "privacy.collect4": "Quality preset and pipeline stage timings", "privacy.neverCollect": "Never collects", "privacy.never1": "Videos, images, or Gaussian splat files", "privacy.never2": "File names, paths, or project names", "privacy.never3": "Project contents, logs, or command output", "privacy.never4": "User names or other personal information", "privacy.uuid": "Each device receives one random UUID. Hardware serial numbers, MAC addresses, and device fingerprints are never read.", "privacy.noEndpoint": "This build has no telemetry endpoint configured, so it will not send telemetry requests.", "privacy.debug": "Telemetry debug mode is active: redacted JSON is written locally and no network request is sent.", "privacy.decline": "No thanks", "privacy.saving": "Saving…", "privacy.share": "Share anonymous statistics", "privacy.analytics": "Anonymous usage statistics", "privacy.analyticsHint": "Share anonymous usage and performance data to improve OOOSplat. Media, file paths, and personal information are never collected.", "privacy.collectSummary": "Collects:", "privacy.collectSummaryText": "Version, system, architecture, anonymous install ID, generation result, and stage timings.", "privacy.noCollectSummary": "Does not collect:", "privacy.noCollectSummaryText": "Videos, images, Gaussian files, file names, paths, project contents, or personal information.", "privacy.network": "Network status:", "privacy.networkOff": "No telemetry endpoint is configured; no request will be sent.", "privacy.networkDebug": "Debug mode; only redacted JSON is written locally.",
  "dialog.largeSequenceTitle": "Large image sequence", "dialog.selectVideoTitle": "Select video file", "dialog.selectImagesTitle": "Select image-sequence folder", "dialog.selectProjectRootTitle": "Select projects root", "dialog.savePlyTitle": "Export Gaussian PLY", "dialog.largeSequence": "This folder contains {count} images. Exhaustive matching time and database size grow quadratically and processing may take a long time.\n\nContinue generation?", "dialog.continue": "Continue", "dialog.deleteTitle": "Delete project", "dialog.deleteProject": "Move “{name}” and all source media, input frames, COLMAP data, Brush data, and logs to the Recycle Bin?\n\nThis cannot be undone inside the app.", "dialog.trash": "Move to Recycle Bin", "dialog.editRecoveryTitle": "Edit data needs recovery", "dialog.editRecovery": "{detail}\n\nYou can clear the damaged crop and deletion data, then reopen the project. The original final.ply will not be changed.", "dialog.clearEdits": "Clear edit data",
  "viewer.aria": "Gaussian splat preview", "viewer.back": "Back to tasks", "viewer.title": "03 Preview", "viewer.modeAria": "Preview workspace mode", "viewer.adjust": "Adjust", "viewer.animation": "Animation", "viewer.adjustHint": "Adjust the Gaussian splat origin, scale, and position", "viewer.animationHint": "Compose the view and export a showcase video", "viewer.inputAria": "View mouse controls", "viewer.left": "Left", "viewer.middle": "Middle", "viewer.right": "Right", "viewer.wheel": "Wheel", "viewer.rotate": "Rotate", "viewer.drag": "Pan", "viewer.zoom": "Zoom", "viewer.select": "Select", "viewer.add": "Add", "viewer.remove": "Remove", "viewer.delete": "Delete", "viewer.cancelSelection": "Clear selection", "viewer.toolsAria": "Gaussian editing tools", "viewer.transform": "Transform", "viewer.rectangle": "Rectangle", "viewer.sphere": "Sphere", "viewer.box": "Box", "viewer.viewsAria": "Orthographic views", "viewer.side": "Side", "viewer.front": "Front", "viewer.top": "Top", "viewer.switchView": "Switch to {view} view", "viewer.undo": "Undo", "viewer.undoTitle": "Undo (Ctrl+Z)", "viewer.redo": "Redo", "viewer.redoTitle": "Redo (Ctrl+Shift+Z / Ctrl+Y)", "viewer.deleteSelected": "Delete selected", "viewer.resetAll": "Reset all", "viewer.resetAllTitle": "Restore the original final.ply", "viewer.save": "Save", "viewer.saveProgress": "Saving {value}%", "viewer.replay": "Replay", "viewer.cancelExport": "Cancel export", "viewer.exportVideo": "Export portrait video", "viewer.preparingExport": "Preparing export", "viewer.rendering": "Rendering {current} / {total}", "viewer.packaging": "Packaging MP4", "viewer.saving": "Saving", "viewer.resourceNote": "Preview and generation are sharing graphics resources. Interaction may slow down if VRAM is limited.", "viewer.initializing": "Initializing WebGL2 renderer", "viewer.mounting": "Creating Gaussian splat GPU resources", "viewer.loading": "Reading Gaussian splat file", "viewer.phaseInitializing": "Initializing", "viewer.phaseLoading": "Loading", "viewer.phaseMounting": "Preparing", "viewer.phaseReady": "Ready", "viewer.phaseError": "Error", "viewer.unavailable": "Preview unavailable", "viewer.reload": "Reload", "viewer.freezing": "Applying crop result", "viewer.wait": "Please wait", "viewer.freezeFailed": "Could not apply crop result", "viewer.backToEdit": "Back to editing", "viewer.splats": "Splat count", "viewer.selected": "Selected", "viewer.deleted": "Deleted", "viewer.fileSize": "File size", "viewer.position": "Position", "viewer.rotation": "Rotation", "viewer.scale": "Scale", "viewer.timeline": "Timeline", "viewer.timelineValue": "Reveal 5s · Shockwave 8s · Orbit 24s / turn", "viewer.renderer": "Renderer", "viewer.status": "Status", "viewer.project": "Project", "viewer.saveFailed": "Save failed", "viewer.unsaved": "Unsaved", "viewer.exported": "Exported", "viewer.videoEncoding": "Video encoding", "viewer.checking": "Checking", "viewer.h264Ready": "H.264 available", "viewer.retrySave": "Retry save", "viewer.exportFailed": "Video export failed: {detail}", "viewer.saveQuestion": "Save edited result?", "viewer.saveDescription": "The current adjustment has not been saved as edit.ply. Crop and deletion data will remain in the project, and the original final.ply will not be modified.", "viewer.skipSave": "Don't save", "viewer.saveContinue": "Save and continue", "viewer.keepOpen": "Keep this window open", "viewer.framesProgress": "{percent}% · {current} / {total} frames", "viewer.orbitAxis": "Orbit axis Y · 24 seconds/turn", "viewer.stateSaved": "Saved", "viewer.stateSaving": "Saving", "viewer.stateFailed": "Save failed", "viewer.stateDirty": "Unsaved", "viewer.contextLost": "The WebGL2 graphics context was lost. A large model may exceed the size of a single graphics allocation supported by this GPU or driver. Close other graphics apps and reload.", "viewer.invalidBounds": "The PLY loaded, but its model bounds are invalid. Confirm that it is a Brush Gaussian PLY generated by OOOSplat.", "viewer.initTimeout": "WebGL2 renderer initialization timed out. Update the graphics driver and Microsoft Edge WebView2 Runtime, then retry.", "viewer.cameraNotReady": "The preview camera is not ready.", "viewer.selectorNotReady": "The Gaussian selector is not ready.", "viewer.textureCapacity": "This GPU supports textures up to {maximum}, but this model requires at least {required}. Preview resources cannot be created safely.", "video.insecure": "This WebView is not a secure context, so the H.264 video encoder is unavailable.", "video.noWebCodecs": "This system WebView does not support WebCodecs VideoEncoder.", "video.noAvc": "No AVC / H.264 encoder is available on this system.", "video.capabilityError": "Could not check H.264 encoding support: {detail}", "video.invalidCanvas": "The video composition canvas must be 1080 × 1920.", "video.noCanvas": "Could not create the video composition canvas.", "video.empty": "The H.264 encoder returned an empty file.", "video.cancelled": "Video export cancelled.", "video.gsplatUnavailable": "The PlayCanvas GSplat system is unavailable.", "video.sortTimeout": "Timed out while waiting for Gaussian splat sorting. Reduce system load and retry.", "video.exportBusy": "A video export is already running.", "video.textureCapacity": "The GPU maximum texture size of {maximum} cannot export a 1080 × 1920 video.", "viewer.contextReload": "The graphics context was lost. Reload the preview.", "viewer.maskRead": "Could not read the deletion mask (HTTP {status})", "viewer.maskLength": "The edit mask length does not match the backend expectation", "viewer.editsUnsaved": "Edit state has not finished saving. Retry before exporting.", "viewer.captureUnavailable": "Could not read the video framing guide. Exit preview and try again.", "viewer.watermarkLoad": "Could not load the OOOSplat watermark logo.", "viewer.cameraUnavailable": "The preview camera component is unavailable", "viewer.captureSize": "The video framing guide has an invalid size. Resize the window and retry.", "viewer.captureOutside": "The video framing guide is outside the rendered image. Resize the window and retry.", "viewer.captureFov": "Could not calculate the camera field of view for the video framing guide.", "viewer.framePixels": "Video frame pixel data is incomplete.", "viewer.cropChanged": "The crop region changed. Retry the operation.", "viewer.freezeMaskLength": "The frozen crop mask length is invalid", "viewer.deletedMaskCount": "The deletion mask does not match the current Gaussian count", "viewer.selectionMaskLength": "Selection mask lengths do not match", "viewer.deletedMaskLength": "Deletion mask lengths do not match", "viewer.editTextures": "Could not create Gaussian editing state textures", "viewer.editMaskLength": "The Gaussian edit mask length is invalid", "viewer.selectorDestroyed": "The Gaussian selector has been destroyed", "viewer.cropExpired": "The Gaussian crop result is stale",
  "panel.dragHint": "Drag an axis label to adjust quickly", "panel.scrubTitle": "{name}: hold the left mouse button and drag horizontally; hold Shift for fine adjustment", "panel.scrubAria": "Drag to adjust {name}", "panel.modelTransform": "Model transform", "panel.position": "Position", "panel.rotation": "Rotation", "panel.angle": "Degrees", "panel.scale": "Scale", "panel.uniform": "Uniform", "panel.uniformScale": "Uniform scale", "panel.region": "Selection region", "panel.sphere": "Sphere", "panel.box": "Box", "panel.keepInside": "{kind} · Keep inside", "panel.regionPosition": "Region position {axis}", "panel.boxSize": "Box size {axis}", "panel.radius": "Radius", "panel.size": "Size", "panel.sphereRadius": "Sphere radius", "panel.noCrop": "No crop is active. Enabling it again will create a new {kind} region around the full model.", "panel.enableCrop": "Enable {kind} crop",
  "animation.reveal": "Reveal", "animation.shockwave": "Shockwave", "animation.orbit": "Orbit", "animation.complete": "Complete",
};

const STORAGE_KEY = "ooo-splat-language";
let activeLocale: Locale = "zh-CN";

export function detectSystemLocale(language?: string): Locale {
  const normalized = (language ?? (typeof navigator !== "undefined" ? navigator.language : "en")).replace("_", "-").toLowerCase();
  return normalized.startsWith("zh") ? "zh-CN" : "en";
}

export function readInitialLocale(): Locale {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (saved === "zh-CN" || saved === "en") return saved;
  } catch { /* WebView storage is optional */ }
  return detectSystemLocale();
}

export function getCurrentLocale(): Locale { return activeLocale; }

export function translate(locale: Locale, key: TranslationKey, params: TranslationParams = {}): string {
  const template = (locale === "zh-CN" ? zhCN : en)[key];
  return template.replace(/\{(\w+)\}/g, (_, name: string) => String(params[name] ?? `{${name}}`));
}

type I18nContextValue = {
  locale: Locale;
  t: (key: TranslationKey, params?: TranslationParams) => string;
  toggleLocale: () => void;
  formatNumber: (value: number) => string;
  formatDate: (value: string | null) => string;
  formatDuration: (milliseconds: number | null) => string;
};

const I18nContext = createContext<I18nContextValue | null>(null);

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(() => {
    const initialLocale = readInitialLocale();
    activeLocale = initialLocale;
    return initialLocale;
  });

  useEffect(() => {
    activeLocale = locale;
    document.documentElement.lang = locale;
  }, [locale]);

  const value = useMemo<I18nContextValue>(() => ({
    locale,
    t: (key, params) => translate(locale, key, params),
    toggleLocale: () => setLocale((current) => {
      const next = current === "zh-CN" ? "en" : "zh-CN";
      activeLocale = next;
      try { window.localStorage.setItem(STORAGE_KEY, next); } catch { /* keep this session language */ }
      return next;
    }),
    formatNumber: (number) => number.toLocaleString(locale),
    formatDate: (date) => date ? new Intl.DateTimeFormat(locale, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }).format(new Date(date)) : "—",
    formatDuration: (milliseconds) => {
      if (milliseconds == null) return "—";
      const seconds = Math.floor(milliseconds / 1000);
      if (locale === "zh-CN") {
        if (seconds < 60) return `${seconds} 秒`;
        if (seconds < 3600) return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
        return `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分`;
      }
      if (seconds < 60) return `${seconds}s`;
      if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
      return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
    },
  }), [locale]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n() {
  const value = useContext(I18nContext);
  if (value) return value;
  return {
    locale: "zh-CN" as const,
    t: (key: TranslationKey, params?: TranslationParams) => translate("zh-CN", key, params),
    toggleLocale: () => undefined,
    formatNumber: (number: number) => number.toLocaleString("zh-CN"),
    formatDate: (date: string | null) => date ? new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }).format(new Date(date)) : "—",
    formatDuration: (milliseconds: number | null) => {
      if (milliseconds == null) return "—";
      const seconds = Math.floor(milliseconds / 1000);
      if (seconds < 60) return `${seconds} 秒`;
      if (seconds < 3600) return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
      return `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分`;
    },
  };
}

const exactPipelineEnglish: Record<string, string> = {
  "正在读取视频信息": "Reading video information",
  "正在规划均匀抽帧": "Planning uniform frame extraction",
  "FFmpeg 正在同步提取透明 PNG 画面与 COLMAP Mask": "FFmpeg is extracting transparent PNG frames and COLMAP masks",
  "FFmpeg 开始提取画面": "FFmpeg started extracting frames",
  "正在分析图片序列": "Analyzing image sequence",
  "正在准备原始图片并生成 COLMAP Alpha Mask": "Preparing source images and COLMAP alpha masks",
  "正在准备图片序列": "Preparing image sequence",
  "已复用特征提取检查点": "Reused feature extraction checkpoint",
  "已复用穷举匹配检查点": "Reused exhaustive matching checkpoint",
  "已复用顺序匹配检查点": "Reused sequential matching checkpoint",
  "已复用相机重建检查点": "Reused camera reconstruction checkpoint",
  "正在增量重建相机轨迹": "Incrementally reconstructing camera poses",
  "增量重建完成": "Incremental reconstruction completed",
  "正在核验注册率和三维点": "Validating registered images and 3D points",
  "已复用 Brush 训练检查点": "Reused Brush training checkpoint",
  "Brush 训练完成": "Brush training completed",
  "正在校验并发布 final.ply": "Validating and publishing final.ply",
  "全部处理完成": "All processing completed",
  "任务已取消": "Task cancelled",
  "正在创建项目": "Creating project",
  "已有任务正在运行": "Another task is already running",
  "任务运行期间不能删除项目": "Projects cannot be deleted while a task is running",
  "项目 ID 无效": "Invalid project ID",
  "该项目已经完成，无需继续": "This project is already complete and does not need to be resumed",
  "项目源素材缺失，无法继续": "The project source media is missing and the task cannot be resumed",
  "项目档位与检查点不一致，无法安全继续": "The project preset does not match its checkpoint and cannot be resumed safely",
  "项目输入类型与检查点不一致，无法安全继续": "The project input type does not match its checkpoint and cannot be resumed safely",
  "项目输入信息不完整": "The project input information is incomplete",
  "COLMAP 未生成完整的稀疏模型": "COLMAP did not produce a complete sparse model",
  "Brush 检查点文件缺失，无法继续发布": "The Brush checkpoint file is missing and the result cannot be published",
  "已有 Gaussian 编辑正在保存": "A Gaussian edit is already being saved",
  "Gaussian 编辑保存令牌无效": "The Gaussian edit save token is invalid",
  "Gaussian 编辑保存会话不存在或已结束": "The Gaussian edit save session does not exist or has ended",
  "Gaussian 编辑位图长度与源文件不一致": "The Gaussian edit mask length does not match the source file",
  "保存期间编辑状态已发生变化，请重新加载": "The edit state changed while saving. Reload and try again",
  "视频编码器返回了空文件。": "The video encoder returned an empty file.",
  "视频文件超过 1 GB 安全限制。": "The video exceeds the 1 GB safety limit.",
  "视频导出令牌无效。": "The video export token is invalid.",
  "视频导出会话不存在或已经结束。": "The video export session does not exist or has ended.",
  "内置 COLMAP 未检测到完整 CUDA 运行时，已使用 CPU": "The bundled COLMAP CUDA runtime is incomplete, so the CPU is being used",
  "无法读取 COLMAP 加速状态，已使用 CPU": "COLMAP acceleration status could not be read, so the CPU is being used",
  "COLMAP 必需命令无法正常启动，不能启用 GPU 加速": "Required COLMAP commands could not start, so GPU acceleration is unavailable",
  "macOS Alpha 当前内置 COLMAP CPU 构建；Brush 仍会使用可用的 Metal 后端": "The macOS Alpha bundles a CPU-only COLMAP build; Brush will still use an available Metal backend",
};

export function localizePipelineMessage(locale: Locale, message: string): string {
  if (locale === "zh-CN") return message;
  const exact = exactPipelineEnglish[message];
  if (exact) return exact;
  const patterns: Array<[RegExp, (...values: string[]) => string]> = [
    [/^预计提取 ([\d,]+) 帧$/, (count) => `About ${count} frames will be extracted`],
    [/^已提取 ([\d,]+) 帧$/, (count) => `Extracted ${count} frames`],
    [/^已提取 ([\d,]+) 张透明 PNG 和 ([\d,]+) 张 Mask$/, (frames, masks) => `Extracted ${frames} transparent PNG frames and ${masks} masks`],
    [/^已准备 ([\d,]+) 张图片$/, (count) => `Prepared ${count} images`],
    [/^已准备 ([\d,]+) 张图片和 ([\d,]+) 张 Mask$/, (images, masks) => `Prepared ${images} images and ${masks} masks`],
    [/^将处理全部 ([\d,]+) 张图片$/, (count) => `All ${count} images will be processed`],
    [/^已复用 ([\d,]+) 帧检查点$/, (count) => `Reused checkpoint with ${count} frames`],
    [/^已发布 ([\d,]+) 个 Splat$/, (count) => `Published ${count} splats`],
    [/^进程已启动 · PID ([\d]+)$/, (pid) => `Process started · PID ${pid}`],
    [/^FFmpeg 已输出 ([\d,]+) 帧$/, (count) => `FFmpeg produced ${count} frames`],
    [/^已注册 ([\d,]+) 张图像$/, (count) => `Registered ${count} images`],
    [/^正在注册第 ([\d,]+) 张图像$/, (count) => `Registering image ${count}`],
    [/^视频 ([\d.]+) 秒 · ([\d.]+) FPS · ([\d]+)×([\d]+)$/, (seconds, fps, width, height) => `Video ${seconds}s · ${fps} FPS · ${width}×${height}`],
    [/^透明视频 ([\d.]+) 秒 · ([\d.]+) FPS · ([\d]+)×([\d]+) · (.+)$/, (seconds, fps, width, height, format) => `Transparent video ${seconds}s · ${fps} FPS · ${width}×${height} · ${format}`],
    [/^图片序列 ([\d,]+) 张 · ([\d]+)×([\d]+)( · 检测到透明区域)?$/, (count, width, height, alpha) => `Image sequence ${count} images · ${width}×${height}${alpha ? " · transparency detected" : ""}`],
    [/^COLMAP 正在使用 (CPU|GPU) 提取特征$/, (backend) => `COLMAP is extracting features with ${backend}`],
    [/^(CPU|GPU) 特征提取完成$/, (backend) => `${backend} feature extraction completed`],
    [/^COLMAP 正在进行 (CPU|GPU) (穷举匹配|顺序匹配)$/, (backend, matcher) => `COLMAP is running ${matcher === "穷举匹配" ? "exhaustive" : "sequential"} matching with ${backend}`],
    [/^(穷举匹配|顺序匹配)完成$/, (matcher) => `${matcher === "穷举匹配" ? "Exhaustive" : "Sequential"} matching completed`],
    [/^文件读写失败：(.+)$/, (detail) => `File I/O failed: ${detail}`],
    [/^外部进程执行失败：(.+)$/, (detail) => `External process failed: ${detail}`],
    [/^视频无效：(.+)$/, (detail) => `Invalid video: ${detail}`],
    [/^图片序列分析任务失败：(.+)$/, (detail) => `Image-sequence analysis failed: ${detail}`],
    [/^图片序列准备任务失败：(.+)$/, (detail) => `Image-sequence preparation failed: ${detail}`],
    [/^找不到本地处理引擎：(.+)$/, (detail) => `Local processing engine not found: ${detail}`],
    [/^当前引擎版本不支持安全接入：(.+)$/, (detail) => `This engine version does not support secure integration: ${detail}`],
    [/^无法解析引擎输出：(.+)$/, (detail) => `Could not parse engine output: ${detail}`],
    [/^注册 ([\d,]+)\/([\d,]+) 张 · 三维点 ([\d,]+)$/, (registered, total, points) => `Registered ${registered}/${total} images · ${points} 3D points`],
    [/^注册率 ([\d.]+)%：低于 80%，将继续训练，但结果质量可能受影响$/, (ratio) => `Registration rate ${ratio}% is below 80%. Training will continue, but result quality may be affected`],
    [/^根据 ([\d,]+) 张输入图片和质量档位估算；完成任务后会自动校准$/, (count) => `Estimated from ${count} input images and the quality preset; it will calibrate automatically after completed tasks`],
    [/^根据 ([\d,]+) 张输入图片、质量档位和本机 ([\d,]+) 个历史任务校准$/, (count, samples) => `Calibrated from ${count} input images, the quality preset, and ${samples} local completed tasks`],
    [/^根据输入 ([\d,]+) 总帧、预计处理 ([\d,]+) 帧和质量档位估算；完成任务后会自动校准$/, (total, planned) => `Estimated from ${total} source frames, ${planned} planned frames, and the quality preset; it will calibrate automatically after completed tasks`],
    [/^根据输入 ([\d,]+) 总帧、预计处理 ([\d,]+) 帧、质量档位和本机 ([\d,]+) 个(.+)任务校准$/, (total, planned, samples, group) => `Calibrated from ${total} source frames, ${planned} planned frames, the quality preset, and ${samples} local ${group === "同档位、相近帧数" ? "same-preset tasks with a similar frame count" : group === "同档位" ? "same-preset tasks" : "cross-preset tasks"}`],
  ];
  for (const [pattern, formatter] of patterns) {
    const match = message.match(pattern);
    if (match) return formatter(...match.slice(1));
  }
  return message;
}
