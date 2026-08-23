# OCI Executor Protocol

Rex compiles typed external-tool requests into `ToolExecutionPlan` values and
then resolves them to executor-neutral `OciJob` values. Docker is the only
production backend shipped by Rex. The protocol exists so a host can add a
remote OCI service without changing Rex modules or exposing host execution.

An `OciJob` contains only:

- a host-selected OCI image and target `os/architecture[/variant]`;
- a fixed catalog command and symbolic arguments;
- declared CAS blob or tree inputs and declared output slots;
- mandatory resource and result limits; and
- an explicit isolation policy.

It cannot carry a developer-machine path, arbitrary mount, environment
override, backend option, device, or secret.

## Logical workspace and CAS transfer

Input and output paths are logical slot numbers. A backend chooses its private
filesystem paths or service objects after dispatch. Declared blob inputs are
transferred by bytes. Tree inputs are transferred recursively while preserving
entry names, kinds, and BLAKE3 identities.

After completion, a backend returns only hashes assigned to declared output
slots. The caller verifies their kinds, total size, and CAS identities before
making them visible to the workflow. Missing completion records, undeclared
slots, wrong object kinds, changed hashes, excessive output, and malformed
provenance are result-protocol failures. A nonzero tool exit remains an
ordinary `ToolExecution` result.

## Required capabilities

`OciExecutorCapabilities` declares controls a target can actually enforce.
The shared validator rejects a job before dispatch if the target cannot provide
any requested guarantee. The secure default requires:

- disabled networking;
- a read-only image root and read-only inputs;
- non-root execution, dropped capabilities, and no-new-privileges behavior;
- no devices, secrets, or additional mounts; and
- execution, stream, output, temporary-storage, memory, CPU, and PID limits.

A managed container product that cannot enforce a required control is not a
conforming target for that job. An adapter must return `Unsupported`; it must
never silently weaken policy.

## Platform, images, and provenance

The executor reports its target platform. Every selected image must target that
exact platform. Production image configuration requires a digest-qualified OCI
reference; mutable tags are restricted to explicit local image development.

Every successful OCI execution includes `ToolExecutionProvenance` identifying
the executor, target platform, immutable image digest, declared input hashes,
and output hashes. This record makes the execution and data boundary auditable;
it is not service attestation by itself.

## Implementing a backend

A provider adapter implements `OciJobExecutor`. It owns authentication,
submission, scheduling, polling, cancellation, private workspace or object
storage, CAS transfer, and service cleanup. It must call or reproduce the
shared validation contract before starting work and must pass
`oci_executor_conformance`.

`FakeRemoteOciExecutor` is an in-memory protocol test double. It uses a CAS
separate from the caller and proves that the boundary does not depend on Docker
bind mounts. It is not a cloud executor: it provides no transport,
authentication, multi-tenant isolation, durable remote storage, or service
attestation.
