#![feature(async_closure)]

#[cfg(feature = "dev")]
mod common;

#[cfg(feature = "dev")]
mod tests {
    use indoc::indoc;
    use json_patch::{PatchOperation, RemoveOperation};
    use k8s_openapi::api::core::v1::Namespace;
    use kube::{
        api::{Patch, PatchParams, PostParams},
        Api, Client, ResourceExt,
    };
    use kubizone_crds::v1alpha1::{Record, Zone};
    use serial_test::serial;

    use crate::common::*;

    #[tokio::test]
    #[serial]
    async fn main() {
        crate::common::run(
            "record-delegation-withdrawn",
            async move |client: Client, namespace: Namespace| {
                // Allows delegation of all record types records to all namespaces.
                let zone: Zone = serde_yaml::from_str(indoc! { r#"
                    apiVersion: dev.kubi.zone/v1alpha1
                    kind: Zone
                    metadata:
                        name: example-org
                    spec:
                        domainName: example.org.
                        delegations:
                            - records:
                                - pattern: "*"
                "# })
                .unwrap();

                // Will be adopted by the example-org zone and included in its list of zone entries.
                let record: Record = serde_yaml::from_str(indoc! { r#"
                    apiVersion: dev.kubi.zone/v1alpha1
                    kind: Record
                    metadata:
                        name: www-example-org
                    spec:
                        domainName: www.example.org.
                        type: A
                        rdata: "192.168.0.2"
                "# })
                .unwrap();

                let zones = Api::<Zone>::namespaced(client.clone(), &namespace.name_any());
                zones.create(&PostParams::default(), &zone).await.unwrap();

                let records = Api::<Record>::namespaced(client.clone(), &namespace.name_any());
                records
                    .create(&PostParams::default(), &record)
                    .await
                    .unwrap();

                wait_for(
                    &zones,
                    &zone.name_any(),
                    &[has_serial(), has_entry("www.example.org.")],
                )
                .await
                .unwrap();

                wait_for(
                    &records,
                    &record.name_any(),
                    &[has_parent("record-delegation-withdrawn.example-org")],
                )
                .await
                .unwrap();

                // Modify the delegations of the parent zone to no longer include
                // the record.
                zones
                    .patch(
                        &zone.name_any(),
                        &PatchParams::apply("record-delegation-withdrawn"),
                        &Patch::<Zone>::Json(json_patch::Patch(vec![PatchOperation::Remove(
                            RemoveOperation {
                                path: jsonptr::Pointer::new(&["spec", "delegations", "0"]),
                            },
                        )])),
                    )
                    .await
                    .unwrap();

                wait_for(
                    &records,
                    &record.name_any(),
                    &[not(has_parent("record-delegation-withdrawn.example-org"))],
                )
                .await
                .unwrap();
            },
        )
        .await;
    }
}
