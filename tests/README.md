# Integration Tests

These tests deploy:

    * A development version of the Kubizone Custom Resource Definitions (dev.kubi.zone)
    * Zone and Record resources using these development CRDs
    * The Kubizone Operator, but operating only on these development CRDs.

This is to enable users to perform Kubizone development on clusters already running production versions of Kubizone, without interfering.


## Tests

* **record_adoption**: Creates a Zone and a Record with an FQDN matching that of the zone, ensuring that the kubizone operator correctly forces adoption of the record.
* **orphaned_record**: As above, but then deletes the parent zone, and verifies that the record's parent zone label is removed.
* **record_delegation_withdrawn**: Ensures parent-zone labels are removed from records, if the already determined parent zone's delegations no longer matches this record.
* **cross_namespace_adoption**: Creates zone `example.org.` in the namespace `cross-namespace-adoption` allowing delegation to all records in `default` namespace. Subsequently, it creates a `good.example.org.` record in `default` namespace and a `bad.example.org.` record in the `cross_namespace_adoption` namespace, and verifies that only the `good.example.org.` record is adoptd.