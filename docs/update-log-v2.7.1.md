---
title: "v2.7.1: 文档系统全面重构"
date: 2025-06-08
---

# v2.7.1: 文档系统全面重构

此版本专注于对 `bili-sync` 的官方文档进行了一次彻底的审查、重构和内容更新，旨在为用户提供更准确、清晰、易于导航的文档体验。

## 🌟 主要亮点

- **📖 内容与功能完全对齐**: 文档内容已根据最新的 `v2.7.1` 版本功能进行全面更新，移除了所有过时的配置项说明（如 `favorite_list`, `collection_list` 等），并增加了关于如何使用现代化 Web UI 管理视频源的详细指南。

- **🔗 全站链接修复**: 系统性地检查并修复了整个文档站点中的内部链接和锚点，解决了之前版本中存在的路径错误和死链问题，确保页面间导航的流畅性。

- **🖼️ 图片资源验证**: 修复了所有文档页面（包括 `README.md`）中的图片引用路径和格式问题，确保所有功能截图和示意图都能正确显示。

- **📝 新增与完善文档**:
  - **新建**: 为"稍后再看" (`watch_later.md`) 和"番剧" (`bangumi.md`) 功能创建了全新的说明文档。
  - **完善**: 大幅充实了 `quick-start.md`, `submission.md`, `favorite.md`, `collection.md` 等核心页面的内容，使其更具指导性。

- **🧭 导航结构优化**: 重新组织了 VitePress 侧边栏的导航结构，使其更有逻辑性，用户可以更轻松地找到所需信息。

## ✅ 修复列表

- **修复**: `introduction.md` 和 `quick-start.md` 中指向"配置迁移指南"的链接路径错误。
- **修复**: `README.md` 中指向旧仓库 (`amtoaer/bili-sync`) 的徽章和图片链接。
- **修复**: `README.md`, `introduction.md`, `features.md` 中所有图片资源的引用路径和扩展名错误（`.jpg` -> `.webp`, 修正相对路径）。
- **移除**: `quick-start.md` 中指向不存在的 `queue.md` 和 `settings.md` 的无效链接。
- **移除**: `configuration.md` 中关于已废弃的 `favorite_list`, `collection_list`, `submission_list`, `watch_later`, `bangumi` 等 `toml` 配置项的过时内容。
- **修正**: `configuration.md` 中因错误的 TOML 示例导致的页面显示样式问题。

总的来说，v2.7.1 是一次以提升文档质量和用户体验为核心的维护版本。 