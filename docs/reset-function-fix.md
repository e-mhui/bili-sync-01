# 🔧 任务重置功能修复说明

## 🎯 问题描述

用户反馈：**任务完成后无法重置任务**

错误提示：`重置无效 所有任务均成功，无需重置`

### 📊 问题分析

原有的重置逻辑只能重置"失败且达到最大重试次数"的任务（状态值 4-6），而不能重置已经成功的任务（状态值 7）。

**状态值含义：**
- 0-3：失败次数，还可以重试
- 4：达到最大重试次数（STATUS_MAX_RETRY = 0b100）
- 7：任务成功（STATUS_OK = 0b111）

当所有任务都成功时，`reset_failed()` 会返回 false，导致无法重置。

## 🛠️ 解决方案

### 1. 后端改进

#### 新增 `reset_all()` 方法
```rust
// crates/bili_sync/src/utils/status.rs
pub fn reset_all(&mut self) -> bool {
    let mut changed = false;
    for i in 0..N {
        let status = self.get_status(i);
        if status != 0 {
            self.set_status(i, 0);
            changed = true;
        }
    }
    if changed {
        self.set_completed(false);
    }
    changed
}
```

#### API 支持强制重置
```rust
// crates/bili_sync/src/api/handler.rs
pub async fn reset_video(
    Path(id): Path<i32>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Extension(db): Extension<Arc<DatabaseConnection>>,
) -> Result<ApiResponse<ResetVideoResponse>, ApiError> {
    // 检查是否强制重置
    let force_reset = params.get("force")
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(false);
    
    // 根据 force_reset 决定使用哪个重置方法
    if force_reset {
        page_status.reset_all()
    } else {
        page_status.reset_failed()
    }
}
```

### 2. 前端改进

#### API 调用支持强制参数
```typescript
// web/src/lib/api.ts
export async function resetVideo(id: number, force: boolean = false): Promise<ResetVideoResponse> {
    const url = force ? `${BASE_URL}/videos/${id}/reset?force=true` : `${BASE_URL}/videos/${id}/reset`;
    return fetchWithAuth(url, { method: 'POST' });
}
```

#### UI 提供两种重置选项
```svelte
<!-- web/src/lib/components/VideoItem.svelte -->
<Button onclick={() => resetVideoItem(false)} variant="secondary">
    重置失败
</Button>
<Button onclick={() => resetVideoItem(true)} variant="destructive">
    强制重置
</Button>
```

## 🎉 使用效果

### ✅ 两种重置模式

1. **重置失败**（普通重置）
   - 只重置失败达到最大重试次数的任务
   - 适用于只想重试失败任务的场景

2. **强制重置**
   - 重置所有任务，包括成功的任务
   - 适用于想重新下载所有内容的场景

### 📋 提示信息优化

| 场景 | 提示信息 |
|------|----------|
| 普通重置成功 | "重置成功，已重置视频与视频的 X 条 page." |
| 强制重置成功 | "重置成功，已重置视频与视频的 X 条 page. (强制重置)" |
| 普通重置无效 | "重置无效，所有任务均成功，无需重置。如需重新下载，请使用强制重置。" |
| 强制重置无任务 | "无任务可重置，该视频暂无任何任务" |

## 💡 技术细节

### 🔧 状态管理

**Status 结构体的状态管理：**
- 使用 u32 存储所有子任务状态
- 每 3 位表示一个子任务状态（最多支持 10 个子任务）
- 最高位（第 31 位）表示整体完成标记

**VideoStatus 包含 5 个子任务：**
1. 视频封面
2. 视频信息
3. Up主头像
4. Up主信息
5. 分P下载

**PageStatus 包含 5 个子任务：**
1. 视频封面
2. 视频内容
3. 视频信息
4. 视频弹幕
5. 视频字幕

### 🛡️ 安全性

- 强制重置需要用户明确选择
- 使用不同的按钮样式区分操作危险程度
- 所有操作都有明确的提示信息

## 🚀 更新说明

**版本：** 修复于 2025-12-11

**影响范围：**
- 后端 API 兼容性：完全兼容，新增可选参数
- 前端界面：新增强制重置按钮
- 数据库：无需迁移

**使用方法：**
1. 更新到最新版本
2. 在视频列表中，每个视频现在有两个重置按钮
3. "重置失败"按钮执行普通重置
4. "强制重置"按钮重置所有任务 