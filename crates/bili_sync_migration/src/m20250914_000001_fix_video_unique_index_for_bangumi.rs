use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 删除旧的唯一索引
        manager
            .drop_index(Index::drop().name("idx_video_unique").to_owned())
            .await?;

        // 创建新的包含 ep_id 的唯一索引
        // 这解决了番剧同一BV号不同集数的插入问题
        manager
            .create_index(
                Index::create()
                    .name("idx_video_unique")
                    .table(Video::Table)
                    .col(Video::CollectionId)
                    .col(Video::FavoriteId)
                    .col(Video::WatchLaterId)
                    .col(Video::SubmissionId)
                    .col(Video::SourceId)
                    .col(Video::Bvid)
                    .col(Video::EpId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 删除新索引
        manager
            .drop_index(Index::drop().name("idx_video_unique").to_owned())
            .await?;

        // 恢复旧的不包含 ep_id 的唯一索引
        manager
            .create_index(
                Index::create()
                    .name("idx_video_unique")
                    .table(Video::Table)
                    .col(Video::CollectionId)
                    .col(Video::FavoriteId)
                    .col(Video::WatchLaterId)
                    .col(Video::SubmissionId)
                    .col(Video::SourceId)
                    .col(Video::Bvid)
                    .unique()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Video {
    Table,
    CollectionId,
    FavoriteId,
    WatchLaterId,
    SubmissionId,
    SourceId,
    Bvid,
    EpId,
}