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

.PHONY: box
box:
	sh scripts/box "$(BOX)" "$(LANE)"

.PHONY: assembly
assembly:
	sh scripts/assembly "$(A)"
