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
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest scripts/box scripts/assembly oracle/source-oracle

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

.PHONY: assembly
assembly:
	sh scripts/assembly "$(A)"

.PHONY: capability-probes
capability-probes:
	python3 -B oracle/probes/capability/run.py

.PHONY: persona-inventory
persona-inventory:
	python3 -B oracle/probes/persona/run.py

.PHONY: zlib-harness
zlib-harness:
	mkdir -p .work/tmp/zlib
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/zlib manganese-oracle-builder:156.0.8078.79-amd64-9535972e6a8a python3 -B /lane/oracle/harness/zlib/build.py

.PHONY: box-fuzz
box-fuzz:
	test "$(BOX)" = zlib
	RUSTC_WRAPPER=kache CARGO_TARGET_DIR=$(CURDIR)/.work/tmp/target TMPDIR=$(CURDIR)/.work/tmp cargo build --locked --offline --target x86_64-unknown-linux-musl -p zlib
	PYTHONPATH=oracle/harness/zlib python3 -B oracle/harness/zlib/fuzz.py

.PHONY: zlib-anchor
zlib-anchor:
	PYTHONPATH=oracle/harness/zlib python3 -B oracle/harness/zlib/anchor.py
