<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import VideoCard from '$lib/components/video-card.svelte';
	import Pagination from '$lib/components/pagination.svelte';
	import SearchBar from '$lib/components/search-bar.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import FilterIcon from '@lucide/svelte/icons/filter';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import { toast } from 'svelte-sonner';
	import api from '$lib/api';
	import type { VideoInfo } from '$lib/types';
	import type { VideosResponse, VideoSourcesResponse, ApiError } from '$lib/types';
	import { VIDEO_SOURCES } from '$lib/consts';
	import {
		appStateStore,
		resetCurrentPage,
		setAll,
		setCurrentPage,
		setQuery,
		setShowFailedOnly,
		setSort,
		ToQuery
	} from '$lib/stores/filter';
	import type { SortBy, SortOrder } from '$lib/types';

	const pageSize = 20;

	let videosData: VideosResponse | null = null;
	let videoSources: VideoSourcesResponse | null = null;
	let loading = false;
	let lastSearch: string | null = null;

	// 重置对话框
	let resetAllDialogOpen = false;
	let resettingAll = false;
	let forceReset = false;

	// 重置任务类型选项
	let resetAllTasks = true;
	let resetTaskPages = false;
	let resetTaskVideo = false;
	let resetTaskInfo = false;
	let resetTaskDanmaku = false;
	let resetTaskSubtitle = false;

	// 筛选状态
	let showFilters = false;
	let selectedSourceType = '';
	let selectedSourceId = '';
	let showFailedOnly = false;
	let currentSortBy: SortBy = 'id';
	let currentSortOrder: SortOrder = 'desc';

	function getApiParams(searchParams: URLSearchParams) {
		let videoSource = null;
		for (const source of Object.values(VIDEO_SOURCES)) {
			const value = searchParams.get(source.type);
			if (value) {
				videoSource = { type: source.type, id: value };
			}
		}
		return {
			query: searchParams.get('query') || '',
			videoSource,
			pageNum: parseInt(searchParams.get('page') || '0'),
			showFailedOnly: searchParams.get('show_failed_only') === 'true',
			sortBy: (searchParams.get('sort_by') as SortBy) || 'id',
			sortOrder: (searchParams.get('sort_order') as SortOrder) || 'desc'
		};
	}

	async function loadVideos(
		query: string,
		pageNum: number = 0,
		filter?: { type: string; id: string } | null,
		showFailedOnly: boolean = false,
		sortBy: SortBy = 'id',
		sortOrder: SortOrder = 'desc'
	) {
		loading = true;
		try {
			const params: Record<string, string | number | boolean> = {
				page: pageNum,
				page_size: pageSize,
				sort_by: sortBy,
				sort_order: sortOrder
			};
			if (query) {
				params.query = query;
			}
			if (filter) {
				params[filter.type] = parseInt(filter.id);
			}
			if (showFailedOnly) {
				params.show_failed_only = true;
			}

			const result = await api.getVideos(params);
			videosData = result.data;
		} catch (error) {
			console.error('加载视频失败:', error);
			toast.error('加载视频失败', {
				description: (error as ApiError).message
			});
		} finally {
			loading = false;
		}
	}

	async function loadVideoSources() {
		try {
			const result = await api.getVideoSources();
			videoSources = result.data;
		} catch (error) {
			console.error('加载视频源失败:', error);
		}
	}

	async function handlePageChange(pageNum: number) {
		setCurrentPage(pageNum);
		goto(`/videos?${ToQuery($appStateStore)}`);
	}

	async function handleSearchParamsChange(searchParams: URLSearchParams) {
		const { query, videoSource, pageNum, showFailedOnly: showFailedOnlyParam, sortBy, sortOrder } = getApiParams(searchParams);
		setAll(query, pageNum, videoSource, showFailedOnlyParam, sortBy, sortOrder);

		// 同步筛选状态
		if (videoSource) {
			selectedSourceType = videoSource.type;
			selectedSourceId = videoSource.id;
		} else {
			selectedSourceType = '';
			selectedSourceId = '';
		}
		showFailedOnly = showFailedOnlyParam;
		currentSortBy = sortBy;
		currentSortOrder = sortOrder;

		loadVideos(query, pageNum, videoSource, showFailedOnlyParam, sortBy, sortOrder);
	}

	async function handleResetVideo(video: VideoInfo, forceReset: boolean) {
		try {
			const result = await api.resetVideo(video.id, forceReset);
			const data = result.data;
			if (data.resetted) {
				toast.success('重置成功', {
					description: `视频「${video.name}」已重置`
				});
				const { query, currentPage, videoSource, showFailedOnly, sortBy, sortOrder } = $appStateStore;
				await loadVideos(query, currentPage, videoSource, showFailedOnly, sortBy, sortOrder);
			} else {
				toast.info('重置无效', {
					description: `视频「${video.name}」没有失败的状态，无需重置`
				});
			}
		} catch (error) {
			console.error('重置失败:', error);
			toast.error('重置失败', {
				description: (error as ApiError).message
			});
		}
	}

	async function handleResetAllVideos() {
		resettingAll = true;
		try {
			let result;
			const { videoSource } = $appStateStore;

			if (resetAllTasks) {
				// 重置所有任务，根据当前过滤器传递参数
				const filterParams = videoSource
					? {
							[videoSource.type]: parseInt(videoSource.id)
						}
					: undefined;
				result = await api.resetAllVideos(filterParams, forceReset);
			} else {
				// 选择性重置特定任务
				const taskIndexes = [];

				// 根据选择的选项确定要重置的任务索引
				// 注意：一个task_index会同时影响VideoStatus和PageStatus的相同索引
				//
				// 后端状态定义：
				// VideoStatus: [视频封面(0), 视频信息(1), Up主头像(2), Up主信息(3), 分P下载(4)]
				// PageStatus: [视频封面(0), 视频内容(1), 视频信息(2), 视频弹幕(3), 视频字幕(4)]
				//
				// 最终修复的索引映射关系：
				// index 0: Video封面 + Page封面 → 封面图片文件
				// index 1: Video信息(普通视频) + Page内容 → 视频文件(.mp4)，番剧无NFO副作用
				// index 2: Video信息(番剧tvshow.nfo) + Page信息 → tvshow.nfo + 单集NFO文件
				// index 3: Video Up主信息 + Page弹幕 → Up主信息 + 弹幕文件(.ass)
				// index 4: Video 分P下载 + Page字幕 → 分P下载 + 字幕文件

				if (resetTaskPages) taskIndexes.push(0); // 重置封面文件
				if (resetTaskVideo) taskIndexes.push(1); // 重置视频内容 (纯视频文件，番剧无NFO)
				if (resetTaskInfo) taskIndexes.push(2); // 重置视频信息 (tvshow.nfo + 单集NFO)
				if (resetTaskDanmaku) taskIndexes.push(3); // 重置弹幕文件 (弹幕 + Up主信息)
				if (resetTaskSubtitle) taskIndexes.push(4); // 重置字幕文件 (字幕 + 分P下载)

				// 去重任务索引
				const uniqueTaskIndexes = [...new Set(taskIndexes)];

				if (uniqueTaskIndexes.length === 0) {
					toast.error('请至少选择一个要重置的任务');
					return;
				}

				// 调用选择性重置API，根据当前过滤器传递参数
				const filterParams = videoSource
					? {
							[videoSource.type]: parseInt(videoSource.id)
						}
					: undefined;
				result = await api.resetSpecificTasks(uniqueTaskIndexes, filterParams, forceReset);
			}

			const data = result.data;
			if (data.resetted) {
				toast.success('重置成功', {
					description: `已重置 ${data.resetted_videos_count} 个视频和 ${data.resetted_pages_count} 个分页`
				});
				// 延迟重新加载视频列表，避免与toast提示冲突
				setTimeout(async () => {
					const { query, currentPage, videoSource: currentVideoSource, showFailedOnly, sortBy, sortOrder } = $appStateStore;
					await loadVideos(query, currentPage, currentVideoSource, showFailedOnly, sortBy, sortOrder);
				}, 100);
			} else {
				toast.info('没有需要重置的视频');
			}
		} catch (error) {
			console.error('重置失败:', error);
			toast.error('重置失败', {
				description: (error as ApiError).message
			});
		} finally {
			resettingAll = false;
			resetAllDialogOpen = false;
		}
	}

	function handleSourceFilter(sourceType: string, sourceId: string) {
		selectedSourceType = sourceType;
		selectedSourceId = sourceId;
		setAll('', 0, { type: sourceType, id: sourceId }, showFailedOnly, currentSortBy, currentSortOrder);
		goto(`/videos?${ToQuery($appStateStore)}`);
	}

	function clearFilters() {
		selectedSourceType = '';
		selectedSourceId = '';
		showFailedOnly = false;
		currentSortBy = 'id';
		currentSortOrder = 'desc';
		setAll('', 0, null, false, 'id', 'desc');
		goto('/videos');
	}
	
	function handleSortChange(sortBy: SortBy, sortOrder: SortOrder) {
		currentSortBy = sortBy;
		currentSortOrder = sortOrder;
		setSort(sortBy, sortOrder);
		resetCurrentPage();
		goto(`/videos?${ToQuery($appStateStore)}`);
	}

	// 处理重置任务选择
	function handleResetAllTasksChange() {
		if (resetAllTasks) {
			resetTaskPages = false;
			resetTaskVideo = false;
			resetTaskInfo = false;
			resetTaskDanmaku = false;
			resetTaskSubtitle = false;
		}
	}

	function handleSpecificTaskChange() {
		if (
			resetTaskPages ||
			resetTaskVideo ||
			resetTaskInfo ||
			resetTaskDanmaku ||
			resetTaskSubtitle
		) {
			resetAllTasks = false;
		}
	}

	$: if ($page.url.search !== lastSearch) {
		lastSearch = $page.url.search;
		handleSearchParamsChange($page.url.searchParams);
	}

	$: totalPages = videosData ? Math.ceil(videosData.total_count / pageSize) : 0;

	onMount(() => {
		setBreadcrumb([{ label: '视频管理' }]);
		loadVideoSources();
	});
</script>

<svelte:head>
	<title>视频管理 - Bili Sync</title>
</svelte:head>

<div class="space-y-6">
	<!-- 搜索和筛选栏 -->
	<div class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
		<div class="max-w-md flex-1">
			<SearchBar
				placeholder="搜索视频标题..."
				value={$appStateStore.query}
				onSearch={(value) => {
					setQuery(value);
					resetCurrentPage();
					goto(`/videos?${ToQuery($appStateStore)}`);
				}}
			/>
		</div>

		<div class="flex items-center gap-2">
			<!-- 排序下拉框 -->
			<div class="flex items-center gap-2">
				<select 
					class="h-9 rounded-md border border-input bg-background px-3 py-1 text-sm ring-offset-background focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
					value="{currentSortBy}_{currentSortOrder}"
					onchange={(e) => {
						const [sortBy, sortOrder] = e.currentTarget.value.split('_') as [SortBy, SortOrder];
						handleSortChange(sortBy, sortOrder);
					}}
				>
					<option value="id_desc">最新添加</option>
					<option value="id_asc">最早添加</option>
					<option value="name_asc">名称 (A-Z)</option>
					<option value="name_desc">名称 (Z-A)</option>
					<option value="upper_name_asc">UP主 (A-Z)</option>
					<option value="upper_name_desc">UP主 (Z-A)</option>
					<option value="created_at_desc">创建时间 (最新)</option>
					<option value="created_at_asc">创建时间 (最早)</option>
				</select>
			</div>
			
			<!-- 筛选按钮 -->
			<Button
				variant={showFilters ? 'default' : 'outline'}
				size="sm"
				onclick={() => (showFilters = !showFilters)}
			>
				<FilterIcon class="mr-2 h-4 w-4" />
				筛选
			</Button>

			<!-- 显示错误视频按钮 -->
			<Button
				variant={showFailedOnly ? 'destructive' : 'outline'}
				size="sm"
				onclick={() => {
					showFailedOnly = !showFailedOnly;
					setShowFailedOnly(showFailedOnly);
					resetCurrentPage();
					goto(`/videos?${ToQuery($appStateStore)}`);
				}}
			>
				只显示错误视频
			</Button>

			<!-- 批量重置按钮 -->
			<Button
				variant="outline"
				size="sm"
				onclick={() => (resetAllDialogOpen = true)}
				disabled={resettingAll || loading}
			>
				<RotateCcwIcon class="mr-2 h-4 w-4 {resettingAll ? 'animate-spin' : ''}" />
				批量重置
			</Button>
		</div>
	</div>

	<!-- 筛选面板 -->
	{#if showFilters && videoSources}
		<div class="space-y-3 rounded-lg border p-3">
			<div class="flex items-center justify-between">
				<h3 class="text-sm font-medium">按视频源筛选</h3>
				{#if selectedSourceType}
					<Button variant="ghost" size="sm" onclick={clearFilters}>清除筛选</Button>
				{/if}
			</div>

			<div class="space-y-3">
				{#each Object.entries(VIDEO_SOURCES) as [sourceKey, sourceConfig] (sourceKey)}
					{@const sources = videoSources[sourceConfig.type]}
					{#if sources && sources.length > 0}
						<div class="space-y-2">
							<div class="flex items-center gap-2">
								<sourceConfig.icon class="text-muted-foreground h-4 w-4" />
								<span class="text-sm font-medium">{sourceConfig.title}</span>
								<Badge variant="outline" class="text-xs">{sources.length}</Badge>
							</div>
							<div class="flex flex-wrap gap-1">
								{#each sources as source (source.id)}
									<Button
										variant={selectedSourceType === sourceConfig.type &&
										selectedSourceId === source.id.toString()
											? 'default'
											: 'outline'}
										size="sm"
										class="h-7 text-xs {!source.enabled ? 'opacity-60' : ''}"
										onclick={() => handleSourceFilter(sourceConfig.type, source.id.toString())}
									>
										{source.name}
										{#if !source.enabled}
											<span class="ml-1 text-xs opacity-70">(禁用)</span>
										{/if}
									</Button>
								{/each}
							</div>
						</div>
					{/if}
				{/each}
			</div>
		</div>
	{/if}

	<!-- 当前筛选状态 -->
	{#if (selectedSourceType && selectedSourceId && videoSources) || showFailedOnly}
		<div class="flex items-center gap-2 flex-wrap">
			<span class="text-muted-foreground text-sm">当前筛选:</span>
			
			{#if selectedSourceType && selectedSourceId && videoSources}
				{@const sourceConfig = Object.values(VIDEO_SOURCES).find(
					(config) => config.type === selectedSourceType
				)}
				{@const sources = videoSources[selectedSourceType]}
				{@const currentSource = sources?.find((s) => s.id.toString() === selectedSourceId)}
				{#if sourceConfig && currentSource}
					<Badge variant="secondary" class="flex items-center gap-1">
						<sourceConfig.icon class="h-3 w-3" />
						{currentSource.name}
						<button onclick={clearFilters} class="hover:bg-muted-foreground/20 ml-1 rounded">
							<span class="sr-only">清除筛选</span>
							×
						</button>
					</Badge>
				{/if}
			{/if}
			
			{#if showFailedOnly}
				<Badge variant="destructive" class="flex items-center gap-1">
					只显示错误视频
					<button 
						onclick={() => {
							showFailedOnly = false;
							setShowFailedOnly(false);
							resetCurrentPage();
							goto(`/videos?${ToQuery($appStateStore)}`);
						}} 
						class="hover:bg-muted-foreground/20 ml-1 rounded"
					>
						<span class="sr-only">清除错误视频筛选</span>
						×
					</button>
				</Badge>
			{/if}
			
			{#if (selectedSourceType && selectedSourceId) || showFailedOnly}
				<Button variant="ghost" size="sm" onclick={clearFilters}>清除所有筛选</Button>
			{/if}
		</div>
	{/if}

	<!-- 视频列表统计 -->
	{#if videosData}
		<div class="text-muted-foreground flex items-center justify-between text-sm">
			<span>
				共 {videosData.total_count} 个视频，当前第 {$appStateStore.currentPage + 1} / {totalPages} 页
			</span>
		</div>
	{/if}

	<!-- 视频卡片网格 -->
	{#if loading}
		<div class="flex items-center justify-center py-16">
			<div class="text-muted-foreground">加载中...</div>
		</div>
	{:else if videosData?.videos.length}
		<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
			{#each videosData.videos as video (video.id)}
				<VideoCard
					{video}
					onReset={async (forceReset) => {
						await handleResetVideo(video, forceReset);
					}}
				/>
			{/each}
		</div>

		<!-- 分页 -->
		{#if totalPages > 1}
			<Pagination
				currentPage={$appStateStore.currentPage}
				{totalPages}
				onPageChange={handlePageChange}
			/>
		{/if}
	{:else}
		<div class="flex items-center justify-center py-16">
			<div class="space-y-2 text-center">
				<div class="text-muted-foreground">暂无视频数据</div>
				<p class="text-muted-foreground text-sm">尝试调整搜索条件或添加视频源</p>
			</div>
		</div>
	{/if}
</div>

<!-- 批量重置确认对话框 -->
<AlertDialog.Root bind:open={resetAllDialogOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>批量重置确认</AlertDialog.Title>
			<AlertDialog.Description>
				{#if selectedSourceType && selectedSourceId && videoSources}
					{@const sourceConfig = Object.values(VIDEO_SOURCES).find(
						(config) => config.type === selectedSourceType
					)}
					{@const sources = videoSources[selectedSourceType]}
					{@const currentSource = sources?.find((s) => s.id.toString() === selectedSourceId)}
					{#if sourceConfig && currentSource}
						确定要重置「{currentSource.name}」视频源下的所有视频状态吗？此操作将清除失败状态并重新开始下载。
					{:else}
						确定要重置当前筛选条件下的所有视频状态吗？此操作将清除失败状态并重新开始下载。
					{/if}
				{:else}
					确定要重置所有视频状态吗？此操作将清除失败状态并重新开始下载。
				{/if}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<div class="space-y-4 py-4">
			<!-- 重置模式选择 -->
			<div class="space-y-2">
				<div class="text-sm font-medium">重置模式：</div>
				<div class="space-y-2 rounded-lg border p-3">
					<label class="flex items-center gap-2">
						<input type="radio" bind:group={forceReset} value={false} />
						<span class="text-sm">只重置失败的任务（推荐）</span>
					</label>
					<label class="flex items-center gap-2">
						<input type="radio" bind:group={forceReset} value={true} />
						<span class="text-sm">强制重置所有任务（包括已完成的）</span>
					</label>
				</div>
			</div>

			<!-- 任务类型选择 -->
			<div class="space-y-3">
				<div class="text-sm font-medium">选择要重置的任务类型：</div>

				<!-- 重置所有任务 -->
				<label class="flex items-center gap-2">
					<input
						type="checkbox"
						bind:checked={resetAllTasks}
						onchange={handleResetAllTasksChange}
						class="rounded border-gray-300"
					/>
					<span class="text-sm font-medium">重置所有任务类型</span>
				</label>

				<!-- 或选择特定任务 -->
				<div class="ml-4 space-y-2">
					<div class="text-muted-foreground text-sm">或选择特定任务：</div>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={resetTaskPages}
							onchange={handleSpecificTaskChange}
							disabled={resetAllTasks}
							class="rounded border-gray-300"
						/>
						<span class="text-sm">重置视频封面</span>
					</label>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={resetTaskVideo}
							onchange={handleSpecificTaskChange}
							disabled={resetAllTasks}
							class="rounded border-gray-300"
						/>
						<span class="text-sm">重置视频内容</span>
					</label>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={resetTaskInfo}
							onchange={handleSpecificTaskChange}
							disabled={resetAllTasks}
							class="rounded border-gray-300"
						/>
						<span class="text-sm">重置视频信息</span>
					</label>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={resetTaskDanmaku}
							onchange={handleSpecificTaskChange}
							disabled={resetAllTasks}
							class="rounded border-gray-300"
						/>
						<span class="text-sm">重置视频弹幕</span>
					</label>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={resetTaskSubtitle}
							onchange={handleSpecificTaskChange}
							disabled={resetAllTasks}
							class="rounded border-gray-300"
						/>
						<span class="text-sm">重置视频字幕</span>
					</label>
				</div>

				<!-- 注意事项 -->
				<div class="mt-4 rounded-lg border border-yellow-200 bg-yellow-50 p-3">
					<div class="text-sm text-yellow-800">
						<strong>说明：</strong>
						<ul class="list-disc list-inside mt-1">
							<li>"只重置失败的任务"模式只会重置状态为失败的任务</li>
							<li>"强制重置"模式会将所有选中的任务重置为"未开始"状态</li>
							<li>选择特定任务类型时，会同时重置对应的分P下载状态</li>
						</ul>
					</div>
				</div>
			</div>
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={resettingAll}>取消</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleResetAllVideos} disabled={resettingAll}>
				{resettingAll ? '重置中...' : '确认重置'}
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
