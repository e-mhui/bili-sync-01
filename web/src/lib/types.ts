// API 响应包装器

export interface ApiResponse<T> {
	status_code: number;
	data: T;
}

// 请求参数类型
export interface VideosRequest {
	collection?: number;
	favorite?: number;
	submission?: number;
	watch_later?: number;
	query?: string;
	page?: number;
	page_size?: number;
}

// 视频来源类型
export interface VideoSource {
	id: number;
	name: string;
	enabled: boolean;
	path: string;
	scan_deleted_videos: boolean;
}

// 视频来源响应类型
export interface VideoSourcesResponse {
	collection: VideoSource[];
	favorite: VideoSource[];
	submission: VideoSource[];
	watch_later: VideoSource[];
	bangumi: VideoSource[];
}

// 视频信息类型
export interface VideoInfo {
	id: number;
	name: string;
	upper_name: string;
	path: string;
	category: number;
	download_status: [number, number, number, number, number];
	cover: string;
}

// 视频列表响应类型
export interface VideosResponse {
	videos: VideoInfo[];
	total_count: number;
}

// 分页信息类型
export interface PageInfo {
	id: number;
	pid: number;
	name: string;
	download_status: [number, number, number, number, number];
	path?: string;
}

// 单个视频响应类型
export interface VideoResponse {
	video: VideoInfo;
	pages: PageInfo[];
}

// 重置视频响应类型
export interface ResetVideoResponse {
	resetted: boolean;
	video: number;
	pages: number[];
}

// 批量重置所有视频响应类型
export interface ResetAllVideosResponse {
	resetted: boolean;
	resetted_videos_count: number;
	resetted_pages_count: number;
}

// 错误类型枚举
export enum ErrorType {
	Network = 'Network',
	Permission = 'Permission',
	Authentication = 'Authentication',
	Authorization = 'Authorization',
	NotFound = 'NotFound',
	RateLimit = 'RateLimit',
	ServerError = 'ServerError',
	ClientError = 'ClientError',
	Parse = 'Parse',
	Timeout = 'Timeout',
	FileSystem = 'FileSystem',
	Configuration = 'Configuration',
	RiskControl = 'RiskControl',
	Unknown = 'Unknown'
}

// 错误类型的中文描述
export const ErrorTypeMessages: Record<ErrorType, string> = {
	[ErrorType.Network]: '网络连接错误',
	[ErrorType.Permission]: '权限不足',
	[ErrorType.Authentication]: '认证失败',
	[ErrorType.Authorization]: '授权失败',
	[ErrorType.NotFound]: '资源未找到',
	[ErrorType.RateLimit]: '请求过于频繁',
	[ErrorType.ServerError]: '服务器内部错误',
	[ErrorType.ClientError]: '客户端错误',
	[ErrorType.Parse]: '解析错误',
	[ErrorType.Timeout]: '超时错误',
	[ErrorType.FileSystem]: '文件系统错误',
	[ErrorType.Configuration]: '配置错误',
	[ErrorType.RiskControl]: '风控触发',
	[ErrorType.Unknown]: '未知错误'
};

// 分类后的错误信息
export interface ClassifiedError {
	error_type: ErrorType;
	message: string;
	status_code?: number;
	should_retry: boolean;
	should_ignore: boolean;
	user_friendly_message?: string;
}

// API 错误类型
export interface ApiError extends ClassifiedError {
	status?: number;
	timestamp?: string;
	request_id?: string;
}

// 添加视频源请求类型
export interface AddVideoSourceRequest {
	source_type: string;
	source_id: string;
	up_id?: string;
	name: string;
	path: string;
	collection_type?: string;
	media_id?: string;
	ep_id?: string;
	download_all_seasons?: boolean;
	selected_seasons?: string[];
}

// 添加视频源响应类型
export interface AddVideoSourceResponse {
	success: boolean;
	message: string;
	source_id?: number;
}

// 删除视频源响应类型
export interface DeleteVideoSourceResponse {
	success: boolean;
	message: string;
}

// 删除视频响应类型
export interface DeleteVideoResponse {
	success: boolean;
	video_id: number;
	message: string;
}

// 配置响应类型
export interface ConfigResponse {
	video_name: string;
	page_name: string;
	multi_page_name?: string;
	bangumi_name?: string;
	folder_structure: string;
	collection_folder_mode?: string;
	time_format: string;
	interval: number;
	nfo_time_type: string;
	parallel_download_enabled: boolean;
	parallel_download_threads: number;
	// 新增视频质量设置
	video_max_quality?: string;
	video_min_quality?: string;
	audio_max_quality?: string;
	audio_min_quality?: string;
	codecs?: string[];
	no_dolby_video?: boolean;
	no_dolby_audio?: boolean;
	no_hdr?: boolean;
	no_hires?: boolean;
	// 新增弹幕设置
	danmaku_duration?: number;
	danmaku_font?: string;
	danmaku_font_size?: number;
	danmaku_width_ratio?: number;
	danmaku_horizontal_gap?: number;
	danmaku_lane_size?: number;
	danmaku_float_percentage?: number;
	danmaku_bottom_percentage?: number;
	danmaku_opacity?: number;
	danmaku_bold?: boolean;
	danmaku_outline?: number;
	danmaku_time_offset?: number;
	// 新增并发控制设置
	concurrent_video?: number;
	concurrent_page?: number;
	rate_limit?: number;
	rate_duration?: number;
	// 新增其他设置
	cdn_sorting?: boolean;
	// 时区设置
	timezone?: string;
	// UP主投稿风控配置
	large_submission_threshold?: number;
	base_request_delay?: number;
	large_submission_delay_multiplier?: number;
	enable_progressive_delay?: boolean;
	max_delay_multiplier?: number;
	enable_incremental_fetch?: boolean;
	incremental_fallback_to_full?: boolean;
	enable_batch_processing?: boolean;
	batch_size?: number;
	batch_delay_seconds?: number;
	enable_auto_backoff?: boolean;
	auto_backoff_base_seconds?: number;
	auto_backoff_max_multiplier?: number;
	// 扫描已删除视频设置
	scan_deleted_videos?: boolean;
	// B站凭证信息
	credential?: {
		sessdata: string;
		bili_jct: string;
		buvid3: string;
		dedeuserid: string;
		ac_time_value: string;
	};
}

// 更新配置请求类型
export interface UpdateConfigRequest {
	video_name?: string;
	page_name?: string;
	multi_page_name?: string;
	bangumi_name?: string;
	folder_structure?: string;
	collection_folder_mode?: string;
	time_format?: string;
	interval?: number;
	nfo_time_type?: string;
	parallel_download_enabled?: boolean;
	parallel_download_threads?: number;
	// 新增视频质量设置
	video_max_quality?: string;
	video_min_quality?: string;
	audio_max_quality?: string;
	audio_min_quality?: string;
	codecs?: string[];
	no_dolby_video?: boolean;
	no_dolby_audio?: boolean;
	no_hdr?: boolean;
	no_hires?: boolean;
	// 新增弹幕设置
	danmaku_duration?: number;
	danmaku_font?: string;
	danmaku_font_size?: number;
	danmaku_width_ratio?: number;
	danmaku_horizontal_gap?: number;
	danmaku_lane_size?: number;
	danmaku_float_percentage?: number;
	danmaku_bottom_percentage?: number;
	danmaku_opacity?: number;
	danmaku_bold?: boolean;
	danmaku_outline?: number;
	danmaku_time_offset?: number;
	// 新增并发控制设置
	concurrent_video?: number;
	concurrent_page?: number;
	rate_limit?: number;
	rate_duration?: number;
	// 新增其他设置
	cdn_sorting?: boolean;
	// 时区设置
	timezone?: string;
	// UP主投稿风控配置
	large_submission_threshold?: number;
	base_request_delay?: number;
	large_submission_delay_multiplier?: number;
	enable_progressive_delay?: boolean;
	max_delay_multiplier?: number;
	enable_incremental_fetch?: boolean;
	incremental_fallback_to_full?: boolean;
	enable_batch_processing?: boolean;
	batch_size?: number;
	batch_delay_seconds?: number;
	enable_auto_backoff?: boolean;
	auto_backoff_base_seconds?: number;
	auto_backoff_max_multiplier?: number;
	// 扫描已删除视频设置
	scan_deleted_videos?: boolean;
}

// 更新配置响应类型
export interface UpdateConfigResponse {
	success: boolean;
	message: string;
	updated_files?: number;
}

// 搜索请求类型
export interface SearchRequest {
	keyword: string;
	search_type: 'video' | 'bili_user' | 'media_bangumi';
	page?: number;
	page_size?: number;
}

// 搜索结果项类型
export interface SearchResultItem {
	result_type: string;
	title: string;
	author: string;
	bvid?: string;
	aid?: number;
	mid?: number;
	season_id?: string;
	media_id?: string;
	cover: string;
	description: string;
	duration?: string;
	pubdate?: number;
	play?: number;
	danmaku?: number;
}

// 搜索响应类型
export interface SearchResponse {
	success: boolean;
	results: SearchResultItem[];
	total: number;
	page: number;
	page_size: number;
}

// 用户收藏夹类型
export interface UserFavoriteFolder {
	id: number | string;
	fid?: number | string;
	name?: string;
	title?: string;
	media_count: number;
	cover?: string;
	created?: number;
}

// 用户合集/系列项类型
export interface UserCollectionItem {
	collection_type: string;
	sid: string;
	name: string;
	cover: string;
	description: string;
	total: number;
	ptime?: number;
	mid: number;
}

// 用户合集响应类型
export interface UserCollectionsResponse {
	success: boolean;
	collections: UserCollectionItem[];
	total: number;
	page: number;
	page_size: number;
}

// 视频分类类型
export type VideoCategory = 'collection' | 'favorite' | 'submission' | 'watch_later' | 'bangumi';

// 番剧季度信息类型
export interface BangumiSeasonInfo {
	season_id: string;
	season_title: string;
	full_title?: string;
	media_id?: string;
	cover?: string;
	episode_count?: number; // 集数
	description?: string; // 简介
}

// 番剧季度响应类型
export interface BangumiSeasonsResponse {
	success: boolean;
	data: BangumiSeasonInfo[];
}

// 关注的UP主信息类型
export interface UserFollowing {
	mid: number;
	name: string;
	face: string;
	sign: string;
	official_verify?: OfficialVerify;
}

// 官方认证信息类型
export interface OfficialVerify {
	type: number;
	desc: string;
}

export interface UserCollectionInfo {
	sid: string;
	name: string;
	cover: string;
	description: string;
	total: number;
	collection_type: string;
	up_name: string;
	up_mid: number;
}

// 队列任务信息类型
export interface QueueTaskInfo {
	task_id: string;
	task_type: string;
	description: string;
	created_at: string;
}

// 队列信息类型
export interface QueueInfo {
	length: number;
	is_processing: boolean;
	tasks: QueueTaskInfo[];
}

// 配置队列信息类型
export interface ConfigQueueInfo {
	update_length: number;
	reload_length: number;
	is_processing: boolean;
	update_tasks: QueueTaskInfo[];
	reload_tasks: QueueTaskInfo[];
}

// 队列状态响应类型
export interface QueueStatusResponse {
	is_scanning: boolean;
	delete_queue: QueueInfo;
	add_queue: QueueInfo;
	config_queue: ConfigQueueInfo;
}

// 状态更新相关类型
export interface StatusUpdate {
	status_index: number;
	status_value: number;
}

export interface PageStatusUpdate {
	page_id: number;
	updates: StatusUpdate[];
}

export interface UpdateVideoStatusRequest {
	video_updates?: StatusUpdate[];
	page_updates?: PageStatusUpdate[];
}

export interface UpdateVideoStatusResponse {
	success: boolean;
	video: VideoInfo;
	pages: PageInfo[];
}

// 更新视频源启用状态请求类型
export interface UpdateVideoSourceEnabledRequest {
	enabled: boolean;
}

// 更新视频源启用状态响应类型
export interface UpdateVideoSourceEnabledResponse {
	success: boolean;
	source_id: number;
	source_type: string;
	enabled: boolean;
	message: string;
}

// 更新视频源扫描已删除视频设置请求类型
export interface UpdateVideoSourceScanDeletedRequest {
	scan_deleted_videos: boolean;
}

// 更新视频源扫描已删除视频设置响应类型
export interface UpdateVideoSourceScanDeletedResponse {
	success: boolean;
	source_id: number;
	source_type: string;
	scan_deleted_videos: boolean;
	message: string;
}

// 重设视频源路径请求类型
export interface ResetVideoSourcePathRequest {
	new_path: string;
	apply_rename_rules?: boolean;
	clean_empty_folders?: boolean;
}

// 重设视频源路径响应类型
export interface ResetVideoSourcePathResponse {
	success: boolean;
	source_id: number;
	source_type: string;
	old_path: string;
	new_path: string;
	moved_files_count: number;
	updated_videos_count: number;
	cleaned_folders_count: number;
	message: string;
}

// 更新凭证请求类型
export interface UpdateCredentialRequest {
	sessdata: string;
	bili_jct: string;
	buvid3: string;
	dedeuserid: string;
	ac_time_value?: string;
}

// 更新凭证响应类型
export interface UpdateCredentialResponse {
	success: boolean;
	message: string;
}

// 初始设置检查响应类型
export interface InitialSetupCheckResponse {
	needs_setup: boolean;
	has_auth_token: boolean;
	has_credential: boolean;
}

// 任务控制响应类型
export interface TaskControlResponse {
	success: boolean;
	message: string;
	is_paused: boolean;
}

// 任务控制状态响应类型
export interface TaskControlStatusResponse {
	is_paused: boolean;
	is_scanning: boolean;
	message: string;
}

// 视频播放信息响应类型
export interface VideoPlayInfoResponse {
	success: boolean;
	video_streams: VideoStreamInfo[];
	audio_streams: AudioStreamInfo[];
	subtitle_streams: SubtitleStreamInfo[];
	video_title: string;
	video_duration?: number;
	video_quality_description: string;
}

// 视频流信息类型
export interface VideoStreamInfo {
	url: string;
	backup_urls: string[];
	quality: number;
	quality_description: string;
	codecs: string;
	width?: number;
	height?: number;
}

// 音频流信息类型
export interface AudioStreamInfo {
	url: string;
	backup_urls: string[];
	quality: number;
	quality_description: string;
}

// 字幕信息类型
export interface SubtitleStreamInfo {
	language: string;
	language_doc: string;
	url: string;
}

// 验证收藏夹响应类型
export interface ValidateFavoriteResponse {
	valid: boolean;
	fid: number;
	title: string;
	message: string;
}
