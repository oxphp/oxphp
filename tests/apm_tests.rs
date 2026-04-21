#[cfg(feature = "plugin-apm")]
mod apm {
    use oxphp::plugins::ox_apm::connection_meta;
    use oxphp::plugins::ox_apm::sql;
    use oxphp::profiling::{ProfilingContext, ProfilingMode, SpanEvent, SpanEventKind};

    #[test]
    fn test_full_request_lifecycle() {
        // Simulate a request: reset → push spans → pop → take
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "trace123".into(), "root456".into());

        // PHP SDK: oxphp_trace_start equivalent
        let s1 = stack.push("user.fetch".into(), vec![("user.id".into(), "42".into())]);

        // Auto-instrumentation: PDO::query equivalent
        let s2 = stack.push(
            "PDO::query".into(),
            vec![
                ("db.system".into(), "mysql".into()),
                (
                    "db.statement".into(),
                    std::sync::Arc::from(
                        sql::obfuscate("SELECT * FROM users WHERE id = 42").as_str(),
                    ),
                ),
                (
                    "db.operation".into(),
                    sql::extract_operation("SELECT * FROM users WHERE id = 42").into(),
                ),
            ],
        );
        stack.pop(s2); // query done

        // PHP SDK: oxphp_trace_end equivalent
        stack.pop(s1);

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 2);

        // PDO span is child of user.fetch span
        let pdo_span = &finished[0];
        assert_eq!(pdo_span.name.as_ref(), "PDO::query");
        assert_eq!(
            pdo_span
                .attributes
                .iter()
                .find(|(k, _)| k.as_ref() == "db.statement")
                .unwrap()
                .1
                .as_ref(),
            "SELECT * FROM users WHERE id = ?"
        );
        assert_eq!(
            pdo_span
                .attributes
                .iter()
                .find(|(k, _)| k.as_ref() == "db.operation")
                .unwrap()
                .1
                .as_ref(),
            "SELECT"
        );
        assert!(!pdo_span.leaked);

        let user_span = &finished[1];
        assert_eq!(user_span.name.as_ref(), "user.fetch");
        assert_eq!(user_span.parent_span_id.as_ref(), "root456");
    }

    #[test]
    fn test_leaked_span_cleanup() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t1".into(), "r1".into());

        stack.push("never_closed".into(), vec![]);
        let leaked = stack.force_close_all();
        assert_eq!(leaked, 1);

        let finished = stack.take_finished();
        assert!(finished[0].leaked);
        assert_eq!(finished[0].name.as_ref(), "never_closed");
    }

    #[test]
    fn test_sql_obfuscation_in_span() {
        let raw = "INSERT INTO orders (user_id, total) VALUES (42, 99.99)";
        let obfuscated = sql::obfuscate(raw);
        assert_eq!(
            obfuscated,
            "INSERT INTO orders (user_id, total) VALUES (?, ?)"
        );
        assert_eq!(sql::extract_operation(raw), "INSERT");
    }

    #[test]
    fn test_connection_meta_lifecycle() {
        connection_meta::clear();

        let meta = connection_meta::parse_pdo_dsn("mysql:host=db.local;port=3306;dbname=app");
        connection_meta::store(100, meta);

        let retrieved = connection_meta::get(100).unwrap();
        assert_eq!(retrieved.db_system, "mysql");
        assert_eq!(retrieved.host, "db.local");
        assert_eq!(retrieved.database, "app");

        connection_meta::clear();
        assert!(connection_meta::get(100).is_none());
    }

    #[test]
    fn test_nested_spans_with_metadata() {
        // Simulate: HTTP handler → DB query → cache lookup
        let mut stack = ProfilingContext::new();
        stack.reset(
            ProfilingMode::ApmOnly,
            "trace_abc".into(),
            "server_span".into(),
        );

        let handler = stack.push("OrderController::show".into(), vec![]);

        let db = stack.push(
            "PDO::query".into(),
            vec![("db.system".into(), "mysql".into())],
        );
        stack.pop(db);

        let cache = stack.push(
            "Redis::get".into(),
            vec![
                ("db.system".into(), "redis".into()),
                ("db.statement".into(), "GET order:42".into()),
            ],
        );
        stack.pop(cache);

        stack.pop(handler);

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 3);

        // Verify parent chain: db and cache are children of handler
        let db_span = &finished[0];
        let cache_span = &finished[1];
        let handler_span = &finished[2];

        assert_eq!(db_span.parent_span_id, handler_span.span_id);
        assert_eq!(cache_span.parent_span_id, handler_span.span_id);
        assert_eq!(handler_span.parent_span_id.as_ref(), "server_span");
    }

    #[test]
    fn test_error_on_span() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t1".into(), "r1".into());

        let s = stack.push("risky_op".into(), vec![]);

        // Simulate exception recording
        if let Some(span) = stack.current_mut() {
            span.status_code = 2; // Error
            span.status_message = Some("connection refused".into());
            span.events.push(SpanEvent {
                name: "exception".into(),
                attributes: vec![
                    ("exception.type".into(), "PDOException".into()),
                    (
                        "exception.message".into(),
                        "SQLSTATE[HY000] [2002] Connection refused".into(),
                    ),
                ],
                timestamp_ns: oxphp::profiling::now_ns(),
                kind: SpanEventKind::Exception,
            });
        }

        stack.pop(s);

        let finished = stack.take_finished();
        assert_eq!(finished[0].status_code, 2);
        assert_eq!(finished[0].events.len(), 1);
        assert_eq!(finished[0].events[0].name, "exception");
    }

    #[test]
    fn test_request_isolation() {
        // Verify spans don't leak between requests
        let mut stack = ProfilingContext::new();

        // Request 1
        stack.reset(ProfilingMode::ApmOnly, "trace_1".into(), "root_1".into());
        stack.push("req1_span".into(), vec![]);
        stack.force_close_all();
        let r1 = stack.take_finished();
        assert_eq!(r1.len(), 1);

        // Request 2
        stack.reset(ProfilingMode::ApmOnly, "trace_2".into(), "root_2".into());
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 0);

        let s = stack.push("req2_span".into(), vec![]);
        stack.pop(s);
        let r2 = stack.take_finished();
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].trace_id.as_ref(), "trace_2");
    }
}
