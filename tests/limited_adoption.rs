#![feature(async_closure)]

#[cfg(feature = "dev")]
mod common;

#[cfg(feature = "dev")]
mod tests {
    use kubizone_common::{Pattern, Type};
    use kubizone_crds::v1alpha1::{Delegation, RecordDelegation};
    use serial_test::serial;

    use crate::common::*;

    #[tokio::test]
    #[serial]
    async fn main() {
        crate::common::run(async move |ctx: Context| {
            ctx.namespace("kubizone-limited-adoption").await.unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-limited-adoption",
                    "example-org",
                    "example.org.",
                    &[Delegation {
                        records: vec![RecordDelegation {
                            pattern: Pattern::try_from("good").unwrap(),
                            types: vec![Type::A],
                        }],
                        namespaces: vec![],
                        zones: vec![],
                    }],
                )
                .await
                .unwrap();

            {
                let good_example_org = ctx
                    .a_record(
                        "kubizone-limited-adoption",
                        "good-a-example-org",
                        "good.example.org.",
                    )
                    .await
                    .unwrap();

                ctx.wait_for(
                    &example_org,
                    &[has_fqdn(), has_serial(), has_entry("good.example.org.")],
                )
                .await
                .unwrap();

                ctx.wait_for(&good_example_org, &[has_fqdn(), has_parent(&example_org)])
                    .await
                    .unwrap();

                ctx.delete(&good_example_org).await.unwrap();

                ctx.wait_for(
                    &example_org,
                    &[
                        has_fqdn(),
                        has_serial(),
                        not(has_entry("good.example.org.")),
                    ],
                )
                .await
                .unwrap();
            }

            {
                let bad_example_org = ctx
                    .a_record(
                        "kubizone-limited-adoption",
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
                    &[has_fqdn(), has_serial(), not(has_entry("bad.example.org."))],
                )
                .await
                .unwrap();
            }

            {
                // This is not adopted because the zone only delegates A records, not AAAA
                let bad_example_org = ctx
                    .record(
                        "kubizone-limited-adoption",
                        "good-aaaa-example-org",
                        "good.example.org.",
                        Type::AAAA,
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
                    &[
                        has_fqdn(),
                        has_serial(),
                        not(has_entry("good.example.org.")),
                    ],
                )
                .await
                .unwrap();
            }
        })
        .await;
    }
}
