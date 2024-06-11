#![feature(async_closure)]

mod common;

//#[cfg(feature = "dev")]
mod tests {
    use indoc::indoc;
    use k8s_openapi::api::core::v1::Namespace;
    use kube::{api::PostParams, Api, Client, ResourceExt};
    use kubizone_crds::v1alpha1::{Record, Zone};

    #[tokio::test]
    async fn main() {
        crate::common::run("simple", async |client: Client, namespace: Namespace| {
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

            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                let created_zone = zones.get(&zone.name_any()).await.unwrap();

                if created_zone.status.iter().any(|status| {
                    status
                        .entries
                        .iter()
                        .any(|entry| &entry.fqdn == "www.example.org.")
                }) {
                    return;
                }
            }
        })
        .await;
    }
}
