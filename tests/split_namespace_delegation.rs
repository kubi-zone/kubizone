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
            ctx.namespace("kubizone-split-root").await.unwrap();
            ctx.namespace("kubizone-split-dev").await.unwrap();
            ctx.namespace("kubizone-split-prod").await.unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-split-root",
                    "example-org",
                    "example.org.",
                    &[
                        Delegation {
                            namespaces: vec!["kubizone-split-dev".to_string()],
                            records: vec![RecordDelegation {
                                pattern: Pattern::try_from("*.dev").unwrap(),
                                types: vec![],
                            }],
                            zones: vec![],
                        },
                        Delegation {
                            namespaces: vec!["kubizone-split-prod".to_string()],
                            records: vec![RecordDelegation {
                                pattern: Pattern::try_from("*").unwrap(),
                                types: vec![],
                            }],
                            zones: vec![],
                        },
                    ],
                )
                .await
                .unwrap();

            // test *.example.org delegation to kubizone-split-prod
            {
                let good_example_org = ctx
                    .record(
                        "kubizone-split-prod",
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

            // test *.example.org non-delegation to kubizone-split-dev
            {
                let bad_example_org = ctx
                    .record("kubizone-split-dev", "bad-dev-example", "bad.example.org.")
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

            // test *.dev.example.org delegation to kubizone-split-dev
            {
                let good_dev_example_org = ctx
                    .record(
                        "kubizone-split-dev",
                        "good-dev-example-org",
                        "good.dev.example.org.",
                    )
                    .await
                    .unwrap();

                ctx.wait_for(
                    &good_dev_example_org,
                    &[has_fqdn(), has_parent(&example_org)],
                )
                .await
                .unwrap();

                ctx.wait_for(
                    &example_org,
                    &[has_serial(), has_fqdn(), has_entry("good.dev.example.org.")],
                )
                .await
                .unwrap();
            }
        })
        .await;
    }
}
