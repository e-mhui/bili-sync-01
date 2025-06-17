# 配置系统

bili-sync v2.7.3 采用全新的配置系统，配置完全存储在数据库中，支持热重载和 API 管理。

## 🆕 配置系统特性

- **数据库存储**：所有配置项存储在 SQLite 数据库中
- **热重载支持**：配置更新后立即生效，无需重启程序
- **API 管理**：通过 RESTful API 管理配置
- **历史记录**：保存配置变更历史，方便审计
- **初始设置向导**：首次启动时提供友好的设置界面

> [!NOTE]
> config.toml 文件仅用于初始配置，程序运行后所有配置更改都保存在数据库中。

## 📚 配置项说明

## video_name 与 page_name

`video_name` 与 `page_name` 用于设置下载文件的命名规则，对于所有下载的内容，将会维持如下的目录结构：

1. 单页视频：

    ```bash
    ├── {video_name}
    │   ├── {page_name}.mp4
    │   ├── {page_name}.nfo
    │   └── {page_name}-poster.jpg
    ```

2. 多页视频：

    ```bash
    ├── {video_name}
    │   ├── poster.jpg
    │   ├── Season 1
    │   │   ├── {page_name} - S01E01.mp4
    │   │   ├── {page_name} - S01E01.nfo
    │   │   ├── {page_name} - S01E01-thumb.jpg
    │   │   ├── {page_name} - S01E02.mp4
    │   │   ├── {page_name} - S01E02.nfo
    │   │   └── {page_name} - S01E02-thumb.jpg
    │   └── tvshow.nfo
    ```

这两个参数支持使用模板，其中用 <code v-pre>{{  }}</code> 包裹的模板变量在执行时会被动态替换为对应的内容。

对于 `video_name`，支持设置 bvid（视频编号）、title（视频标题）、upper_name（up 主名称）、upper_mid（up 主 id）、pubtime（视频发布时间）、fav_time（视频收藏时间）。

对于 `page_name`，除支持 video 的全部参数外，还支持 ptitle（分 P 标题）、pid（分 P 页号）。

为了解决文件名可能过长的问题，程序为模板引入了 `truncate` 函数。如 <code v-pre>{{ truncate title 10 }}</code> 表示截取 `title` 的前 10 个字符。

> [!TIP]
> 1. 仅收藏夹视频会区分 `fav_time` 和 `pubtime`，其它类型下载两者的取值是完全相同的；
> 2. `fav_time` 和 `pubtime` 的格式受 `time_format` 参数控制，详情可参考 [time_format 小节](#time-format)。

此外，`video_name` 和 `page_name` 还支持使用路径分割符，如 <code v-pre>{{ upper_mid }}/{{ title }}_{{ pubtime }}</code> 表示视频会根据 UP 主 id 将视频分到不同的文件夹中。

推荐仅在 `video_name` 中使用路径分割符，暂不清楚在 `page_name` 中使用路径分割符导致分页存储到子文件夹后是否还能被媒体服务器正确识别。

> [!CAUTION]
> **路径分隔符**在不同平台定义不同，Windows 下为 `\`，MacOS 和 Linux 下为 `/`。

> [!TIP]
> v2.7.3 新增：所有文件名都会自动通过 `filenamify` 函数处理，确保跨平台兼容性。特殊字符会被自动转换：
> - 全角冒号 `：` → `-`
> - 日文引号 `「」` → `[]`
> - 全角括号 `（）` → `()`
> - 书名号 `《》` → `_`

### 番剧命名配置

番剧下载支持单独的命名模板配置：

- `bangumi_video_name`: 番剧视频文件夹命名
- `bangumi_page_name`: 番剧分集文件命名

支持的模板变量：
- `season_id`: 季度 ID
- `season_title`: 季度标题
- `ep_id`: 分集 ID
- `ep_index`: 分集序号
- `ep_title`: 分集标题
- `badge`: 集数标识（如"第1话"）

## time_format

`time_format` 参数用于格式化 `pubtime` 和 `fav_time` 的显示格式，使用 strftime 格式化字符串，如 `%Y-%m-%d` 表示 2021-01-01。详细的格式化字符串请参考[这里](https://docs.rs/chrono/0.4.38/chrono/format/strftime/index.html)。

## credential

`credential` 参数用于设置登录凭据，包括 `sessdata`、`bili_jct`、`buvid3`、`dedeuserid` 和 `ac_time_value` 五个字段。

> [!TIP]
> v2.7.3 提供了初始设置向导，首次启动时会引导您设置凭据。

### 如何获取凭据？

1. 登录 B 站
2. 打开浏览器开发者工具（F12）
3. 切换到 Application/存储 标签
4. 在左侧找到 Cookies
5. 复制对应的值

> [!WARNING]
> 凭据信息非常重要，请妥善保管，不要泄露给他人。

## video_quality 与 audio_quality

`video_quality` 与 `audio_quality` 用于设置下载视频的质量上限。前者包括：

- `_360p`: 流畅 360P
- `_480p`: 清晰 480P
- `_720p`: 高清 720P
- `_1080p`: 高清 1080P
- `_1080p_plus`: 高清 1080P60
- `_1080p_60`: 高清 1080P60
- `_4k`: 超清 4K
- `HDR`: HDR 真彩
- `Dolby`: 杜比视界

后者包括：

- `_64k`: 64K
- `_132k`: 132K
- `_192k`: 192K
- `Hi-Res`: Hi-Res 无损
- `Dolby`: 杜比全景声

尽管大部分视频都支持 `_1080p` 及以下的视频质量，但较老的视频或 UP 主设置后可能会没有 `_1080p` 的视频流，因此程序执行的下载策略是下载 **不超过设定质量** 的最高质量的视频流。

## video_format

`video_format` 用于设置下载视频的格式，目前哔哩哔哩 Dash 流支持的格式为 `mp4`、`flv`。**两种格式没有本质区别**，仅推荐 `mp4` 更加通用。

## video_codecs

`video_codecs` 用于设置下载视频的编码优先级，支持的编码包括：

- `AVC`: H.264/AVC 编码
- `HEV`: H.265/HEVC 编码  
- `AV1`: AV1 编码

> [!TIP]
> v2.7.3 改进：编码优先级现在通过拖拽排序设置，更加直观。

默认优先级为 `["HEV", "AVC", "AV1"]`，表示：
1. 优先下载 HEVC 编码的视频
2. 如果没有 HEVC，则下载 AVC
3. 最后选择 AV1

## download_subtitle

`download_subtitle` 用于设置是否下载字幕，如果设置为 `true`，则会下载视频的字幕（视频的 cc 字幕或者投稿字幕）。会保存在视频同目录下，命名为 `{page_name}.srt`。

## 弹幕设置

`danmaku` 参数用于设置是否下载弹幕以及弹幕的渲染参数。相关参数及默认值为：

```toml
[danmaku]
# 开关
enabled = false
# 弹幕密度，-1 表示不限制
density = -1
# 弹幕字号
fontsize = 25
# 弹幕不透明度
opacity = 50
# 是否加粗
bold = false
```

### 详细参数说明

#### `enabled`
是否下载弹幕并保存为 .ass 文件。

#### `density`
弹幕密度，即屏幕上同时显示的最大弹幕数。设置为 -1 表示不限制。

#### `fontsize`
弹幕字体大小。

#### `float_duration`
滚动弹幕的持续时间（秒）。

#### `static_duration`
静止弹幕（顶部/底部弹幕）的持续时间（秒）。

#### `padding`
两条弹幕之间最小的水平距离。

#### `lane_size`
弹幕所占据的高度，即"行高度/行间距"。

#### `float_percentage`
屏幕上滚动弹幕最多高度百分比。

#### `bottom_percentage`
屏幕上底部弹幕最多高度百分比。

#### `opacity`
透明度，取值范围为 0-100。

#### `bold`
是否加粗。

#### `outline`
描边宽度。

#### `time_offset`
时间轴偏移，>0 会让弹幕延后，<0 会让弹幕提前，单位为秒。

## `concurrent_limit`

对 bili-sync 的并发下载进行多方面的限制，避免 api 请求过于频繁导致的风控。其中 video 和 page 表示下载任务的并发数，rate_limit 表示 api 请求的流量限制。默认取值为：
```toml
[concurrent_limit]
video = 3
page = 2

[concurrent_limit.rate_limit]
limit = 4
duration = 250
```

具体来说，程序的处理逻辑是严格从上到下的，即程序会首先并发处理多个 video，每个 video 内再并发处理多个 page，程序的并行度可以简单衡量为 `video * page`（很多 video 都只有单个 page，实际会更接近 `video * 1`），配置项中的 `video` 和 `page` 两个参数就是控制此处的，调节这两个参数可以宏观上控制程序的并行度。

另一方面，每个执行的任务内部都会发起若干 api 请求以获取信息，这些请求的整体频率受到 `rate_limit` 的限制，使用漏桶算法实现。如默认配置表示的是每 250ms 允许 4 个 api 请求，超过这个频率的请求会被暂时阻塞，直到漏桶中有空间为止。调节 `rate_limit` 可以从微观上控制程序的并行度，同时也是最直接、最显著的控制 api 请求频率的方法。

据观察 b 站风控限制大多集中在主站，因此目前 `rate_limit` 仅作用于主站的各类请求，如请求各类视频列表、视频信息、获取流下载地址等，对实际的视频、图片下载过程不做限制。

> [!TIP]
> 1. 一般来说，`video` 和 `page` 的值不需要过大；
> 2. `rate_limit` 的值可以根据网络环境和 api 请求频率进行调整，如果经常遇到风控可以优先调小 limit。

## 其他配置

### interval

`interval` 参数用于设置扫描间隔，单位为秒。程序会每隔 `interval` 秒扫描一次视频源，检查是否有新的视频需要下载。

> [!TIP]
> v2.7.3 改进：扫描间隔修改后会立即生效，无需等待当前周期结束。

### bili_download

`bili_download` 参数配置多线程下载相关设置：

```toml
[bili_download]
# 启用多线程下载的最小文件大小（字节）
min_size = 10485760  # 10MB
# 下载线程数
threads = 3
```

### bind_address

Web 管理界面的绑定地址，默认为 `"0.0.0.0:12345"`。

## 配置管理 API

v2.7.3 提供了完整的配置管理 API：

### 获取配置
```bash
GET /api/config/{key}
```

### 更新配置
```bash
PUT /api/config/{key}
Content-Type: application/json

{
  "value": "new_value"
}
```

### 批量更新
```bash
POST /api/config/batch
Content-Type: application/json

{
  "updates": {
    "key1": "value1",
    "key2": "value2"
  }
}
```

### 触发热重载
```bash
POST /api/config/reload
```

### 查看配置历史
```bash
GET /api/config/history?limit=10
```

## 配置迁移

从旧版本升级时，程序会自动将 config.toml 中的配置迁移到数据库。迁移后：

1. config.toml 仅作为初始配置参考
2. 所有配置更改通过 Web 界面或 API 进行
3. 配置更改立即生效，无需重启

> [!IMPORTANT]
> 视频源配置已完全迁移到数据库，不再存储在 config.toml 中。