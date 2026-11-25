.POSIX:

setup:
	sh scripts/setup-dev

check:
	sh scripts/check

lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest scripts/box scripts/assembly

artifact-selftest:
	sh scripts/artifact-selftest

test: check

ARCH = x86_64
MODE = headless
LANE = seen

oracle-launch:
	python3 -B oracle/launch/launch.py --arch $(ARCH) --mode $(MODE)

box:
	sh scripts/box "$(BOX)" "$(LANE)"

assembly:
	sh scripts/assembly "$(A)"

.PHONY: setup check lint test artifact-selftest oracle-launch box assembly
