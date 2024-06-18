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
            ctx.namespace("kubizone-longest-matching").await.unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-longest-matching",
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

            let sub_example_org = ctx
                .zone(
                    "kubizone-longest-matching",
                    "sub-example-org",
                    "sub.example.org.",
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

            let good_sub_sub_example_org = ctx
                .a_record(
                    "kubizone-longest-matching",
                    "good-sub-sub-www-example-org",
                    "good.sub.sub.example.org.",
                )
                .await
                .unwrap();

            ctx.wait_for(
                &good_sub_sub_example_org,
                &[has_fqdn(), has_parent(&sub_example_org)],
            )
            .await
            .unwrap();

            ctx.wait_for(
                &example_org,
                &[has_serial(), not(has_entry("good.sub.sub.example.org."))],
            )
            .await
            .unwrap();

            ctx.wait_for(
                &sub_example_org,
                &[has_serial(), has_entry("good.sub.sub.example.org.")],
            )
            .await
            .unwrap();
        })
        .await;
    }
}
