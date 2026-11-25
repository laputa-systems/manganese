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

.PHONY: brotli-harness
brotli-harness:
	mkdir -p .work/tmp/brotli
	docker run --rm --mount type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly --mount type=bind,src=$(CURDIR),dst=/lane -e TMPDIR=/lane/.work/tmp/brotli $$(sh oracle/source-oracle image-tag) python3 -B /lane/oracle/harness/brotli/build.py

.PHONY: brotli-anchor
brotli-anchor:
	PYTHONPATH=oracle/harness/brotli python3 -B oracle/harness/brotli/anchor.py
