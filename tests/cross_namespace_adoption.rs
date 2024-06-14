#![feature(async_closure)]

#[cfg(feature = "dev")]
mod common;

#[cfg(feature = "dev")]
mod tests {
    use std::time::Duration;

    use indoc::indoc;
    use k8s_openapi::api::core::v1::Namespace;
    use kube::{api::PostParams, Api, Client, ResourceExt};
    use kubizone_crds::v1alpha1::{Record, Zone};
    use serial_test::serial;

    use crate::common::*;

    #[tokio::test]
    #[serial]
    async fn main() {
        crate::common::run(
            "cross-namespace-adoption",
            async move |client: Client, namespace: Namespace| {
                // Allows delegation of all record types records within the "default" namespace.
                let zone: Zone = serde_yaml::from_str(indoc! { r#"
                    apiVersion: dev.kubi.zone/v1alpha1
                    kind: Zone
                    metadata:
                        name: example-org
                    spec:
                        domainName: example.org.
                        delegations:
                            - namespaces:
                              - default
                              records:
                                - pattern: "*"
                "# })
                .unwrap();

                let zones = Api::<Zone>::namespaced(client.clone(), &namespace.name_any());
                zones.create(&PostParams::default(), &zone).await.unwrap();

                // Will NOT be adopted by the example-org zone, since the zone does not
                // allow delegation to the cross-namespacea-adoption namespace.
                {
                    let bad_record: Record = serde_yaml::from_str(indoc! { r#"
                        apiVersion: dev.kubi.zone/v1alpha1
                        kind: Record
                        metadata:
                            name: bad-example-org
                        spec:
                            domainName: bad.example.org.
                            type: A
                            rdata: "192.168.0.2"
                    "# })
                    .unwrap();

                    let records = Api::<Record>::namespaced(client.clone(), &namespace.name_any());
                    records
                        .create(&PostParams::default(), &bad_record)
                        .await
                        .unwrap();

                    // A little hard to prove a negative in an eventually-consistent
                    // architecture, but give the controller a chance at least.
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    // bad has not been adopted.
                    wait_for(
                        &zones,
                        &zone.name_any(),
                        &[has_serial(), not(has_entry("bad.example.org."))],
                    )
                    .await
                    .unwrap();

                    wait_for(
                        &records,
                        &bad_record.name_any(),
                        &[not(has_parent("cross-namespace-adoption.example-org"))],
                    )
                    .await
                    .unwrap();
                }

                // Will be adopted by the example-org zone, since the zone allows
                // delegation to the default namespace (which this will be placed in).
                {
                    let good_record: Record = serde_yaml::from_str(indoc! { r#"
                        apiVersion: dev.kubi.zone/v1alpha1
                        kind: Record
                        metadata:
                            name: good-example-org
                        spec:
                            domainName: good.example.org.
                            type: A
                            rdata: "192.168.0.3"
                    "# })
                    .unwrap();

                    let records = Api::<Record>::namespaced(client.clone(), "default");
                    records
                        .create(&PostParams::default(), &good_record)
                        .await
                        .unwrap();

                    // good has been adopted.
                    wait_for(
                        &zones,
                        &zone.name_any(),
                        &[has_serial(), has_entry("good.example.org.")],
                    )
                    .await
                    .unwrap();

                    wait_for(
                        &records,
                        &good_record.name_any(),
                        &[has_parent("cross-namespace-adoption.example-org")],
                    )
                    .await
                    .unwrap();
                }
            },
        )
        .await;
    }
}
