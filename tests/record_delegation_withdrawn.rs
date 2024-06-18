#![feature(async_closure)]

#[cfg(feature = "dev")]
mod common;

#[cfg(feature = "dev")]
mod tests {
    use json_patch::{PatchOperation, RemoveOperation};
    use kube::{
        api::{Patch, PatchParams},
        Api, ResourceExt,
    };
    use kubizone_common::Pattern;
    use kubizone_crds::v1alpha1::{Delegation, RecordDelegation, Zone};
    use serial_test::serial;

    use crate::common::*;

    #[tokio::test]
    #[serial]
    async fn main() {
        crate::common::run(async move |ctx: Context| {
            ctx.namespace("kubizone-record-delegation-withdrawn")
                .await
                .unwrap();

            let example_org = ctx
                .zone(
                    "kubizone-record-delegation-withdrawn",
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
                .record(
                    "kubizone-record-delegation-withdrawn",
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

            let api =
                Api::<Zone>::namespaced(ctx.client().await, "kubizone-record-delegation-withdrawn");

            // Delete the record delegation
            api.patch(
                &example_org.name_any(),
                &PatchParams::apply("record-delegation-withdrawn"),
                &Patch::<Zone>::Json(json_patch::Patch(vec![PatchOperation::Remove(
                    RemoveOperation {
                        path: jsonptr::Pointer::new(&["spec", "delegations", "0"]),
                    },
                )])),
            )
            .await
            .unwrap();

            ctx.wait_for(
                &example_org,
                &[has_serial(), not(has_entry("www.example.org."))],
            )
            .await
            .unwrap();

            ctx.wait_for(&www_example_org, &[not(has_parent(&example_org))])
                .await
                .unwrap();
        })
        .await;
    }
}
