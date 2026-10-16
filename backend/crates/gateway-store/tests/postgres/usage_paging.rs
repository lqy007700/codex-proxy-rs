//! 请求列表的精确总数、双向分页与筛选隔离

use chrono::{TimeDelta, Utc};
use gateway_store::postgres::{
    ObservabilityPageSize, ObservabilityRange, OpsErrorFilter, OpsErrorQuery, UsageRecordFilter,
    UsageRecordQuery,
};

use super::{TestDatabase, analytics::seed, observability_repository};

#[tokio::test]
async fn projected_page_counts_correct_pending_moves_deletions_and_model_or() {
    let Some(database) = TestDatabase::create("usage_projected_pages").await else {
        return;
    };
    let start = chrono::DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for index in 0..40 {
        seed(
            &database.pool,
            &format!("projected_{index:02}"),
            start + TimeDelta::minutes(index),
            &[],
        )
        .await;
    }
    super::drain_analytics(&database.pool).await;
    let repository = observability_repository(&database.pool);
    for stage in 0..3 {
        if stage == 1 {
            sqlx::query("update model_requests set started_at=started_at+interval '30 minutes',recovery_lease_expires_at=recovery_lease_expires_at+interval '30 minutes',completed_at=completed_at+interval '30 minutes',requested_model_id='model_b',upstream_model_id='model_b' where id='projected_06'")
                .execute(&database.pool).await.unwrap();
            sqlx::query("delete from model_requests where id='projected_17'")
                .execute(&database.pool)
                .await
                .unwrap();
            sqlx::query("update model_requests set outcome='failed' where id='projected_21'")
                .execute(&database.pool)
                .await
                .unwrap();
            seed(
                &database.pool,
                "projected_new",
                start + TimeDelta::seconds(30),
                &[],
            )
            .await;
        } else if stage == 2 {
            super::drain_analytics(&database.pool).await;
        }
        for edge in [0, 31] {
            let range = ObservabilityRange::new(
                start + TimeDelta::seconds(edge),
                start + TimeDelta::hours(1) - TimeDelta::seconds(edge),
            )
            .unwrap();
            for model in [None, Some("model_a"), Some("model_b")] {
                // 真值只读取实际源版本，不依赖被测汇总或分页辅助函数
                let expected:Vec<String> = sqlx::query_scalar("select id from model_requests where started_at >= $1 and started_at < $2 and outcome='succeeded' and ($3::text is null or requested_model_id=$3 or upstream_model_id=$3) order by started_at desc,id desc")
                    .bind(range.start).bind(range.end).bind(model).fetch_all(&database.pool).await.unwrap();
                for current_page in [1, 3, 6, 8] {
                    let page = repository
                        .list_usage_records(UsageRecordQuery {
                            scope: Default::default(),
                            range,
                            filter: UsageRecordFilter {
                                client_api_key_ref: Some("key_analytics".to_owned()),
                                model: model.map(str::to_owned),
                                ..Default::default()
                            },
                            current_page,
                            page_size: ObservabilityPageSize::new(7).unwrap(),
                        })
                        .await
                        .unwrap();
                    assert_eq!(
                        page.total,
                        expected.len() as u64,
                        "stage={stage}, edge={edge}, model={model:?}"
                    );
                    assert_eq!(
                        page.items.iter().map(|row| &row.id).collect::<Vec<_>>(),
                        expected
                            .iter()
                            .skip((current_page as usize - 1) * 7)
                            .take(7)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
    database.close().await;
}

#[tokio::test]
async fn usage_pages_preserve_order_and_exact_totals_from_both_ends() {
    let Some(database) = TestDatabase::create("usage_bidirectional_pages").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    seed(&database.pool, "page_00", start, &[]).await;
    sqlx::query("insert into model_requests select cloned.* from model_requests original cross join generate_series(1,22) n cross join lateral jsonb_populate_record(null::model_requests,to_jsonb(original)||jsonb_build_object('id','page_'||lpad(n::text,2,'0'),'client_api_key_ref',case when n%2=0 then 'key_analytics' else 'other_key' end)) cloned where original.id='page_00'")
        .execute(&database.pool).await.unwrap();
    let range =
        ObservabilityRange::new(start - TimeDelta::seconds(1), start + TimeDelta::seconds(1))
            .unwrap();
    let repository = observability_repository(&database.pool);
    for key in [
        None,
        Some("key_analytics"),
        Some("other_key"),
        Some("absent_key"),
    ] {
        let expected = (0..23)
            .rev()
            .filter(|n| match key {
                None => true,
                Some("key_analytics") => n % 2 == 0,
                Some("other_key") => n % 2 != 0,
                Some(_) => false,
            })
            .map(|n| format!("page_{n:02}"))
            .collect::<Vec<_>>();
        for page in 1..=5 {
            let result = repository
                .list_usage_records(UsageRecordQuery {
                    scope: Default::default(),
                    range,
                    filter: UsageRecordFilter {
                        client_api_key_ref: key.map(str::to_owned),
                        ..Default::default()
                    },
                    current_page: page,
                    page_size: ObservabilityPageSize::new(7).unwrap(),
                })
                .await
                .unwrap();
            let ids = result
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            let expected_page = expected
                .iter()
                .skip((page as usize - 1) * 7)
                .take(7)
                .map(String::as_str)
                .collect::<Vec<_>>();
            assert_eq!(
                result.total,
                expected.len() as u64,
                "key={key:?}, page={page}"
            );
            assert_eq!(ids, expected_page, "key={key:?}, page={page}");
        }
    }
    database.close().await;
}

#[tokio::test]
async fn usage_pages_cross_minutes_and_keep_exact_counts_after_source_changes() {
    let Some(database) = TestDatabase::create("usage_time_pages").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    for index in 0..29 {
        seed(
            &database.pool,
            &format!("cross_{index:02}"),
            start + TimeDelta::seconds((index / 3) * 100),
            &["group_page"],
        )
        .await;
    }
    let repository = observability_repository(&database.pool);
    let range = ObservabilityRange::new(start, start + TimeDelta::minutes(20)).unwrap();
    for removed in [None, Some("cross_16")] {
        if let Some(id) = removed {
            sqlx::query("delete from model_requests where id = $1")
                .bind(id)
                .execute(&database.pool)
                .await
                .unwrap();
        }
        let expected = (0..29)
            .rev()
            .map(|index| format!("cross_{index:02}"))
            .filter(|id| Some(id.as_str()) != removed)
            .collect::<Vec<_>>();
        for current_page in 1..=6 {
            let page = repository
                .list_usage_records(UsageRecordQuery {
                    scope: Default::default(),
                    range,
                    filter: UsageRecordFilter {
                        client_api_key_ref: Some("key_analytics".to_owned()),
                        search: Some("model_".to_owned()),
                        ..Default::default()
                    },
                    current_page,
                    page_size: ObservabilityPageSize::new(7).unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(page.total, expected.len() as u64);
            assert_eq!(
                page.items.iter().map(|item| &item.id).collect::<Vec<_>>(),
                expected
                    .iter()
                    .skip((current_page as usize - 1) * 7)
                    .take(7)
                    .collect::<Vec<_>>(),
                "page={current_page}, removed={removed:?}",
            );
        }
    }
    database.close().await;
}

#[tokio::test]
async fn ops_pages_merge_sources_across_minutes_with_stable_ties() {
    let Some(database) = TestDatabase::create("ops_time_pages").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    let mut expected = Vec::new();
    for index in 0..17 {
        let id = format!("error_{index:02}");
        let at = start + TimeDelta::seconds((index / 3) * 100);
        seed(&database.pool, &id, at, &[]).await;
        sqlx::query("update model_requests set error_kind = 'upstream_error' where id = $1")
            .bind(&id)
            .execute(&database.pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into ops_events (id, model_request_id, attempt_index, level, component,
                 operation, failure_kind, message, created_at)
             values ($1, $1, 1, 'error', 'routing', 'responses', 'upstream_error', 'fixture', $2)",
        )
        .bind(&id)
        .bind(at + TimeDelta::minutes(1))
        .execute(&database.pool)
        .await
        .unwrap();
        for source in ["model_request", "ops_event"] {
            expected.push((at + TimeDelta::minutes(1), format!("{source}:{id}")));
        }
    }
    expected.sort_by(|a, b| b.cmp(a));
    let repository = observability_repository(&database.pool);
    let range = ObservabilityRange::new(start, start + TimeDelta::minutes(20)).unwrap();
    for current_page in 1..=7 {
        let page = repository
            .list_ops_errors(OpsErrorQuery {
                range,
                filter: OpsErrorFilter {
                    client_api_key_ref: Some("key_analytics".to_owned()),
                    search: Some("error_".to_owned()),
                    ..Default::default()
                },
                current_page,
                page_size: ObservabilityPageSize::new(7).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(page.total, expected.len() as u64);
        assert_eq!(
            page.items
                .iter()
                .map(|item| &item.stable_sort_id)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .skip((current_page as usize - 1) * 7)
                .take(7)
                .map(|(_, id)| id)
                .collect::<Vec<_>>(),
            "page={current_page}",
        );
    }
    database.close().await;
}

#[tokio::test]
async fn request_scopes_page_all_outcomes_without_changing_usage_facts() {
    use gateway_admin::model::observability::UsageRecordScope;
    let Some(database) = TestDatabase::create("request_scopes").await else {
        return;
    };
    let start = Utc::now() - TimeDelta::hours(1);
    let outcomes = [
        "succeeded",
        "failed",
        "cancelled",
        "incomplete",
        "running",
        "succeeded",
    ];
    let attempt_counts = [1_u32, 3, 2, 0, 1, 1];
    for (index, outcome) in outcomes.iter().enumerate() {
        let id = format!("scope_{index}");
        seed(
            &database.pool,
            &id,
            start + TimeDelta::seconds(index as i64),
            &[],
        )
        .await;
        sqlx::query("update model_requests set outcome=$2,attempt_count=$3,provider_kind='openai',provider_account_ref='scope_account',upstream_transport='http_sse',upstream_send_state=case when $3=0 then 'not_sent' else 'sent' end,completed_at=case when $2='running' then null else completed_at end,request_observation_json=request_observation_json || jsonb_build_object('error',jsonb_build_object('message','demo error')),error_details='demo error details' where id=$1")
            .bind(&id).bind(outcome).bind(i32::try_from(attempt_counts[index]).unwrap()).execute(&database.pool).await.unwrap();
    }
    // 成功审计没有模型或用量证据时，不能进入成功用量聚合
    sqlx::query("update model_requests set requested_model_id=null,input_tokens=null,output_tokens=null,total_tokens=null where id='scope_5'")
        .execute(&database.pool).await.unwrap();
    let repository = observability_repository(&database.pool);
    let range = ObservabilityRange::new(start, start + TimeDelta::minutes(5)).unwrap();
    for projected in [false, true] {
        if projected {
            super::drain_analytics(&database.pool).await;
        }
        for (scope, expected) in [
            (UsageRecordScope::Usage, vec!["scope_0"]),
            (
                UsageRecordScope::All,
                vec![
                    "scope_5", "scope_4", "scope_3", "scope_2", "scope_1", "scope_0",
                ],
            ),
            (UsageRecordScope::Success, vec!["scope_5", "scope_0"]),
            (
                UsageRecordScope::Error,
                vec!["scope_3", "scope_2", "scope_1"],
            ),
        ] {
            for current_page in 1..=4 {
                let page = repository
                    .list_usage_records(UsageRecordQuery {
                        scope,
                        range,
                        filter: UsageRecordFilter::default(),
                        current_page,
                        page_size: ObservabilityPageSize::new(2).unwrap(),
                    })
                    .await
                    .unwrap();
                assert_eq!(
                    page.total,
                    expected.len() as u64,
                    "{scope:?}, projected={projected}"
                );
                assert_eq!(
                    page.items
                        .iter()
                        .map(|row| row.id.as_str())
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .copied()
                        .skip((current_page as usize - 1) * 2)
                        .take(2)
                        .collect::<Vec<_>>()
                );
                for row in page.items {
                    let index: usize = row.id.trim_start_matches("scope_").parse().unwrap();
                    assert_eq!(row.outcome.as_str(), outcomes[index]);
                    assert_eq!(row.attempt_count, attempt_counts[index]);
                    assert_eq!(row.error_message.as_deref(), Some("demo error"));
                }
            }
        }
        let searched = repository
            .list_usage_records(UsageRecordQuery {
                scope: UsageRecordScope::Error,
                range,
                filter: UsageRecordFilter {
                    search: Some("scope_1".into()),
                    ..Default::default()
                },
                current_page: 1,
                page_size: ObservabilityPageSize::new(2).unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(searched.total, 1);
        assert_eq!(searched.items[0].id, "scope_1");
    }
    let detail = repository.usage_record_detail("scope_1").await.unwrap();
    assert_eq!(
        detail.request.error_details.as_deref(),
        Some("demo error details")
    );
    let summary = repository
        .usage_summary(range, UsageRecordFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.requests.total_tokens, 120);
    assert_eq!(summary.requests.latency_count, 1);
    database.close().await;
}
