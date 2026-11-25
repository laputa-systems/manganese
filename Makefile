.POSIX:

setup:
	sh scripts/setup-dev

check:
	sh scripts/check

lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest

artifact-selftest:
	sh scripts/artifact-selftest

test: check

ARCH = x86_64
MODE = headless
ARCHIVE = hydrogen-r2-amd64
CLAUSES =

oracle-launch:
	python3 -B oracle/launch/launch.py --arch $(ARCH) --mode $(MODE)

contract:
	python3 -B oracle/launch/contract/contract.py --archive '$(ARCHIVE)' --clauses '$(CLAUSES)'

contract-selftest:
	python3 -B oracle/launch/contract/selftest.py

.PHONY: setup check lint test artifact-selftest oracle-launch contract contract-selftest
