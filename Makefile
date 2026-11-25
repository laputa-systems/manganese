.POSIX:

setup:
	sh scripts/setup-dev

check:
	sh scripts/check

lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest oracle/source-oracle

artifact-selftest:
	sh scripts/artifact-selftest

test: check

ARCH = x86_64
MODE = headless

oracle-launch:
	python3 -B oracle/launch/launch.py --arch $(ARCH) --mode $(MODE)

oracle-gate-a:
	sh oracle/source-oracle image ensure-source gate-a

.PHONY: setup check lint test artifact-selftest oracle-launch oracle-gate-a
