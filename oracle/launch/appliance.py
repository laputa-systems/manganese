"""The appliance's fixed launch environment (docs/appliance-contract.md, clauses 2 and 3)."""

from __future__ import annotations

INSTALL_ROOT = "/opt/browser"
LAUNCHER_LINK = "/usr/local/bin/browser"
USER = "browser"
UID = 1000
GID = 27
HOME = "/home/browser"
PROFILE = "/profile"
DOWNLOADS = "/home/browser/Downloads"
DEVTOOLS_PORT = 9221
START_URL = "about:blank"

ENVIRONMENT = {
    "HOME": HOME,
    "USER": USER,
    "LD_LIBRARY_PATH": f"{INSTALL_ROOT}/lib",
    "FONTCONFIG_FILE": f"{INSTALL_ROOT}/fontconfig/fonts.conf",
    "FONTCONFIG_PATH": f"{INSTALL_ROOT}/fontconfig",
}

COMMON_SWITCHES = (
    f"--remote-debugging-port={DEVTOOLS_PORT}",
    f"--user-data-dir={PROFILE}",
    "--no-first-run",
    "--no-default-browser-check",
)

PROXY_SWITCHES = (
    "--proxy-server=socks5://127.0.0.1:1080",
    "--proxy-bypass-list=<-loopback>",
)

HEADLESS_SWITCHES = (
    "--headless=new",
    "--disable-gpu",
    "--in-process-gpu",
    "--disable-background-timer-throttling",
    "--disable-frame-rate-limit",
    "--animation-duration-scale=0",
    "--wm-window-animations-disabled",
    "--disable-smooth-scrolling",
    "--cc-scroll-animation-duration-in-seconds=0",
    "--force-prefers-reduced-motion",
    "--hide-scrollbars",
    "--disk-cache-dir=/tmp/cache",
    "--disable-breakpad",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--disable-default-apps",
    "--disable-extensions",
    "--disable-component-extensions-with-background-pages",
    "--metrics-recording-only",
    "--no-pings",
    "--password-store=basic",
    "--disable-hang-monitor",
    "--enable-features=NetworkServiceInProcess2,LazyKeyedServiceInstantiation,"
    "LazyKeyedServiceInstantiationExtensions,LazyKeyedServiceInstantiationExtensionsApi,"
    "LazyKeyedServiceInstantiationStorageNotification,LazyKeyedServiceInstantiationVisitedUrlRanking,"
    "LazyKeyedServiceInstantiationOptimizationGuide,LazyKeyedServiceInstantiationAutofillAndPassword,"
    "LazyKeyedServiceInstantiationSafetyHub",
    "--disable-features=SpareRendererForSitePerProcess,OptimizationHints,MediaRouter,"
    "AutofillServerCommunication,SegmentationPlatform,overlay-scrollbars",
)

HEADED_SWITCHES = (
    "--ozone-platform=wayland",
    "--start-maximized",
    "--use-gl=angle",
    "--use-angle=swiftshader",
    "--disable-gpu-compositing",
)

MODE_SWITCHES = {"headless": HEADLESS_SWITCHES, "headed": HEADED_SWITCHES}

# Clause 4.
READINESS_POLL_SECONDS = 0.002
READINESS_DEADLINE_SECONDS = 60.0
READINESS_CONNECT_TIMEOUT_SECONDS = 1.0
READINESS_READ_TIMEOUT_SECONDS = 2.0
CLOSE_DEADLINE_SECONDS = 8.0


def argv(executable: str, mode: str, start_url: str = START_URL, extra: tuple[str, ...] = ()) -> list[str]:
    """The supervisor's command line, in the order the appliance builds it (guest/appliance/src/browser.rs).

    The supervisor resolves LAUNCHER_LINK and executes the resolved file; the start URL is always last (3.1).
    """
    surface, rest = MODE_SWITCHES[mode][:2], MODE_SWITCHES[mode][2:]
    return [executable, *surface, *PROXY_SWITCHES, *rest, *COMMON_SWITCHES, *extra, start_url]
