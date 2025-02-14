use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_cloudwatchlogs::Client as CloudWatchLogsClient;
use aws_lambda_events::event::s3::S3Event;
use lambda_runtime::{run, service_fn, Error as LambdaError, LambdaEvent};
use std::env;
use flate2::read::GzDecoder;
use std::io::Read;
use chrono::{DateTime as ChronoDateTime, Utc};

async fn function_handler(event: LambdaEvent<S3Event>) -> Result<(), LambdaError> {
    let region_provider = RegionProviderChain::default_provider();
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
    let s3_client = S3Client::new(&config);
    let cloudwatch_logs = CloudWatchLogsClient::new(&config);

    // S3イベントからバケット名とキーを取得
    let record = &event.payload.records[0];
    let bucket = record.s3.bucket.name.as_ref().unwrap();
    let key = record.s3.object.key.as_ref().unwrap();

    // S3からログファイルを取得
    let resp = s3_client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    // ロググループ名を環境変数から取得
    let log_group_name = env::var("LOG_GROUP_NAME").unwrap_or_else(|_| "ALB-access-log".to_string());

    // ログストリーム名を日付から生成（YYYY-MM-DD形式）
    let now: ChronoDateTime<Utc> = Utc::now();
    let log_stream_name = now.format("%Y-%m-%d").to_string();

    // ログストリームを作成（存在しない場合）
    match cloudwatch_logs
        .create_log_stream()
        .log_group_name(&log_group_name)
        .log_stream_name(&log_stream_name)
        .send()
        .await
    {
        Ok(_) => (),
        Err(err) => {
            if !err.to_string().contains("ResourceAlreadyExistsException") {
                return Err(err.into());
            }
        }
    }

    // S3からログファイルを取得して解凍
    let body = resp.body.collect().await?.to_vec();
    let mut gz = GzDecoder::new(&body[..]);
    let mut content = String::new();
    gz.read_to_string(&mut content)?;

    // 各行をそのままCloudWatch Logsに送信
    for line in content.lines() {
        cloudwatch_logs
            .put_log_events()
            .log_group_name(&log_group_name)
            .log_stream_name(&log_stream_name)
            .log_events(
                aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
                    .message(line.to_string())
                    .timestamp(now.timestamp_millis())
                    .build()?
            )
            .send()
            .await?;
    }

    Ok(())
}

fn main() -> Result<(), LambdaError> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .without_time()
        .init();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(service_fn(function_handler)))
}
