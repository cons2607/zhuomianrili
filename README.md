# 桌面日历

一款轻量级 Windows 桌面日历应用，支持活动管理、智能提醒、OCR 文字识别和主题换肤。

## 功能特性

- **日历视图**：月视图展示，快速查看活动安排
- **智能解析**：粘贴文本自动识别日期、时间、活动标题
- **OCR 识别**：截图或图片文字识别，快速创建活动
- **重复事件**：支持每日、每周、每月、每年重复
- **到期提醒**：活动前一天 07:30 自动提醒，支持自定义时间
- **开机补提醒**：关机期间错过的提醒，开机后自动补弹
- **主题换肤**：从照片提取主色调，全局自动换肤
- **花园组件**：完成任务获得浇水，培养小植物
- **轻量内存**：托盘 ~27MB，开窗 ~128MB（硬线 200MB 内）

## 技术栈

- **前端**：Vue 3 + Vite
- **后端**：Rust + Tauri 2
- **OCR**：Windows 系统 OCR（WinRT）
- **提醒**：Windows 原生 Toast 通知
- **安装包**：NSIS

## 下载安装

从 [Releases](https://github.com/yourusername/desktop-calendar/releases) 下载最新安装包 `桌面日历_x64-setup.exe`。

安装程序会将应用安装到 `%LOCALAPPDATA%\DesktopCalendar\`，无需管理员权限。

## 开发

### 环境要求

- Rust 1.70+
- Node.js 18+
- Windows 10/11

### 安装依赖

```bash
npm install
```

### 开发模式

```bash
npm run dev
```

然后在另一个终端运行：

```bash
cd src-tauri
cargo run
```

### 构建安装包

```bash
npm run tauri build
```

安装包生成在 `src-tauri/target/release/bundle/nsis/`。

## 数据说明

- 所有数据存储在本地 `data.json`（与 exe 同目录）
- 无账号、无云端、断网可用
- 数据完全由用户掌控

## 许可证

本项目仅供学习和个人使用。
