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
            ctx.namespace("kubizone-local-namespace").await.unwrap();
            ctx.namespace("kubizone-foreign-namespace").await.unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-local-namespace",
                    "example-org",
                    "example.org.",
                    &[Delegation {
                        namespaces: vec!["kubizone-foreign-namespace".to_string()],
                        records: vec![RecordDelegation {
                            pattern: Pattern::try_from("*").unwrap(),
                            types: vec![],
                        }],
                        zones: vec![],
                    }],
                )
                .await
                .unwrap();

            // No delegation is configured for records in kubizone-local-namespace
            // so the following record won't be adopted.
            {
                let bad_example_org = ctx
                    .a_record(
                        "kubizone-local-namespace",
                        "bad-example-org",
                        "bad.example.org.",
                    )
                    .await
                    .unwrap();

                ctx.wait_for(
                    &bad_example_org,
                    &[has_fqdn(), not(has_parent(&example_org))],
                )
                .await
                .unwrap();

                ctx.wait_for(
                    &example_org,
                    &[has_serial(), has_fqdn(), not(has_entry("bad.example.org."))],
                )
                .await
                .unwrap();
            }

            {
                let good_example_org = ctx
                    .a_record(
                        "kubizone-foreign-namespace",
                        "good-example-org",
                        "good.example.org.",
                    )
                    .await
                    .unwrap();
                ctx.wait_for(&good_example_org, &[has_fqdn(), has_parent(&example_org)])
                    .await
                    .unwrap();

                ctx.wait_for(
                    &example_org,
                    &[has_serial(), has_fqdn(), has_entry("good.example.org.")],
                )
                .await
                .unwrap();
            }
        })
        .await;
    }
}
