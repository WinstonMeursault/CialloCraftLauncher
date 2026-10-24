use ccl_core::content::ContentKind;
use ccl_gui::instance_service::InstanceRequest;
use serde_json::json;

#[test]
fn request_wire_format_is_tagged_and_rejects_unknown_fields() {
    let request: InstanceRequest = serde_json::from_value(json!({"operation":"content_add","id":"test","kind":"resource_pack","source":"/tmp/pack.zip","name":null})).unwrap();
    assert!(matches!(
        request,
        InstanceRequest::ContentAdd {
            kind: ContentKind::ResourcePack,
            ..
        }
    ));
    assert!(
        serde_json::from_value::<InstanceRequest>(
            json!({"operation":"show","id":"test","extra":true})
        )
        .is_err()
    );
}

#[test]
fn query_and_preview_requests_are_read_only() {
    for request in [
        InstanceRequest::List,
        InstanceRequest::Show { id: "x".into() },
        InstanceRequest::History { id: "x".into() },
        InstanceRequest::DiskUsage { id: "x".into() },
        InstanceRequest::Clean {
            id: "x".into(),
            keep: 2,
            include_previous: false,
            apply: false,
        },
        InstanceRequest::PackInspect {
            archive: "/tmp/a.mrpack".into(),
        },
        InstanceRequest::PackShow { id: "x".into() },
        InstanceRequest::ContentList {
            id: "x".into(),
            kind: ContentKind::Mod,
        },
    ] {
        assert!(!request.mutates());
    }
}

#[test]
fn destructive_and_apply_requests_are_mutations() {
    for request in [
        InstanceRequest::Delete { id: "x".into() },
        InstanceRequest::Clean {
            id: "x".into(),
            keep: 2,
            include_previous: true,
            apply: true,
        },
        InstanceRequest::PackExport {
            id: "x".into(),
            output: "/tmp/a.mrpack".into(),
            include_local_content: false,
        },
        InstanceRequest::ContentRestore {
            id: "x".into(),
            archive: "/tmp/a.zip".into(),
        },
    ] {
        assert!(request.mutates());
    }
}
