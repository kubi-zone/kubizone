#![feature(async_closure)]

#[cfg(feature = "dev")]
mod common;

#[cfg(feature = "dev")]
mod tests {
    use kubizone_common::Pattern;
    use kubizone_crds::v1alpha1::{Delegation, RecordDelegation};
    use serial_test::serial;

    use crate::common::*;

    #[tokio::test]
    #[serial]
    async fn main() {
        crate::common::run(async move |ctx: Context| {
            ctx.namespace("kubizone-record-adoption").await.unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-record-adoption",
                    "example-org",
                    "example.org.",
                    &[Delegation {
                        records: vec![RecordDelegation {
                            pattern: Pattern::try_from("*").unwrap(),
                            types: vec![],
                        }],
                        namespaces: vec![],
                        zones: vec![],
                    }],
                )
                .await
                .unwrap();

            let www_example_org = ctx
                .a_record(
                    "kubizone-record-adoption",
                    "www-example-org",
                    "www.example.org.",
                )
                .await
                .unwrap();

            ctx.wait_for(
                &example_org,
                &[has_fqdn(), has_serial(), has_entry("www.example.org.")],
            )
            .await
            .unwrap();

            ctx.wait_for(&www_example_org, &[has_fqdn(), has_parent(&example_org)])
                .await
                .unwrap();
        })
        .await;
    }
}
