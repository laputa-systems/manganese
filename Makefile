.POSIX:

ARCH = x86_64
MODE = headless
LANE = seen
ARCHIVE = hydrogen-r2-amd64
CLAUSES =

.PHONY: setup
setup:
	sh scripts/setup-dev

.PHONY: check
check:
	sh scripts/check

.PHONY: lint
lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest scripts/box scripts/accept scripts/assembly oracle/source-oracle

.PHONY: test
test: check

.PHONY: artifact-selftest
artifact-selftest:
	sh scripts/artifact-selftest

.PHONY: oracle-launch
oracle-launch:
	python3 -B oracle/launch/launch.py --arch $(ARCH) --mode $(MODE)

.PHONY: oracle-gate-a
oracle-gate-a:
	sh oracle/source-oracle image ensure-source gate-a

.PHONY: contract
contract:
	python3 -B oracle/launch/contract/contract.py --archive '$(ARCHIVE)' --clauses '$(CLAUSES)'

.PHONY: contract-selftest
contract-selftest:
	python3 -B oracle/launch/contract/selftest.py

.PHONY: census
census:
	python3 -B oracle/probes/census/census.py

.PHONY: box
box:
	sh scripts/box "$(BOX)" "$(LANE)"

.PHONY: accept
accept:
	sh scripts/accept "$(BOX)"

.PHONY: box-fuzz
box-fuzz:
	SEED="$(SEED)" CASES="$(CASES)" INJECT="$(INJECT)" sh scripts/box "$(BOX)" fuzz

.PHONY: assembly
assembly:
	sh scripts/assembly "$(A)"

.PHONY: capability-probes
capability-probes:
	python3 -B oracle/probes/capability/run.py $(PROBE_ARGS)

.PHONY: persona-inventory
persona-inventory:
	python3 -B oracle/probes/persona/run.py $(PROBE_ARGS)

.PHONY: oracle-equivalence
oracle-equivalence:
	python3 -B oracle/probes/equivalence/run.py --rebuild '$(REBUILD)' $(EQUIVALENCE_ARGS)

.PHONY: zlib-harness
zlib-harness:
	mkdir -p .work/tmp/zlib
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/zlib $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/zlib/build.py

.PHONY: numerics-harness
numerics-harness:
	mkdir -p .work/tmp/numerics
	set -e; image=$$(sh oracle/source-oracle image-tag); docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/numerics "$$image" python3 -B /lane/oracle/harness/numerics/build_json.py
	set -e; image=$$(sh oracle/source-oracle image-tag); docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/numerics "$$image" python3 -B /lane/oracle/harness/numerics/build.py

.PHONY: url-harness
url-harness:
	mkdir -p .work/tmp/url
	set -e; image=$$(sh oracle/source-oracle image-tag); docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/url "$$image" python3 -B /lane/oracle/harness/url/build.py

.PHONY: checksums-harness
checksums-harness:
	mkdir -p .work/tmp/checksums
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/checksums $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/checksums/build.py
.PHONY: runtime-harness
runtime-harness:
	mkdir -p .work/tmp/runtime
	docker run --rm --network none --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/runtime $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/runtime/build.py
.PHONY: zstd-harness
zstd-harness:
	mkdir -p .work/tmp/zstd
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/zstd $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/zstd/build.py

.PHONY: png-harness
png-harness:
	mkdir -p .work/tmp/png
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/png $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/png/build.py

.PHONY: png-anchor
png-anchor:
	python3 -B oracle/harness/png/anchor.py

.PHONY: zlib-anchor
zlib-anchor:
	PYTHONPATH=oracle/harness/zlib python3 -B oracle/harness/zlib/anchor.py
	python3 -B oracle/probes/capability/run.py
.PHONY: abi-census
abi-census:
	python3 -B oracle/probes/abi/census.py
.PHONY: cdp-trace
cdp-trace:
	python3 -B oracle/probes/cdp/run.py

.PHONY: devtools-endpoint-oracle
devtools-endpoint-oracle:
	python3 -B oracle/harness/devtools-endpoint/daemon.py

.PHONY: devtools-endpoint-record
devtools-endpoint-record:
	CARGO_TARGET_DIR=$(CURDIR)/.work/tmp/target RUSTC_WRAPPER=kache cargo build --locked --offline --target x86_64-unknown-linux-musl -p devtools-endpoint
	python3 -B oracle/harness/devtools-endpoint/record.py

.PHONY: brotli-harness
brotli-harness:
	mkdir -p .work/tmp/brotli
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/brotli $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/brotli/build.py

.PHONY: brotli-anchor
brotli-anchor:
	PYTHONPATH=oracle/harness/brotli python3 -B oracle/harness/brotli/anchor.py

.PHONY: leveldb-harness
leveldb-harness:
	mkdir -p .work/tmp/leveldb
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/leveldb $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/leveldb/build.py

.PHONY: leveldb-record
leveldb-record:
	python3 -B oracle/harness/leveldb/record.py

.PHONY: leveldb-anchor
leveldb-anchor:
	python3 -B oracle/harness/leveldb/anchor.py

.PHONY: encoding-harness
encoding-harness:
	mkdir -p .work/tmp/encoding
	tag=$$(sh oracle/source-oracle image-tag) && docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/encoding "$$tag" python3 -B /lane/oracle/harness/encoding/build.py

.PHONY: zstd-anchor
zstd-anchor:
	PYTHONPATH=oracle/harness/zstd python3 -B oracle/harness/zstd/anchor.py

.PHONY: protobuf-harness
protobuf-harness:
	mkdir -p .work/tmp/protobuf
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/protobuf $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/protobuf/build.py

.PHONY: http1-harness
http1-harness:
	mkdir -p .work/tmp/http1
	tag=$$(sh oracle/source-oracle image-tag) && docker run --rm --network none --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/http1 "$$tag" python3 -B /lane/oracle/harness/http1/build.py

.PHONY: http1-record
http1-record:
	python3 -B oracle/harness/http1/record.py

.PHONY: http1-anchor
http1-anchor:
	PYTHONPATH=oracle/harness/http1 python3 -B oracle/harness/http1/anchor.py

.PHONY: cert-verify-harness
cert-verify-harness:
	mkdir -p .work/tmp/cert-verify
	tag=$$(sh oracle/source-oracle image-tag) && docker run --rm --network none --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/cert-verify "$$tag" python3 -B /lane/oracle/harness/cert-verify/build.py

.PHONY: cert-verify-record
cert-verify-record:
	mkdir -p .work/tmp/cert-verify
	tag=$$(sh oracle/source-oracle image-tag) && docker run --rm --network none --user "$$(id -u):$$(id -g)" --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/cert-verify -w /lane/oracle/harness/cert-verify "$$tag" python3 -B cases.py /work/src /lane/.work/tmp/cert-verify/cases.bin
	python3 -B oracle/harness/cert-verify/record.py
