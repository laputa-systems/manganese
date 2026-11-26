#include <stdlib.h>

/* Chromium's env links its remote FilesystemProxy over Mojo, Perfetto's track descriptors and the
   GLib UI message pump; the harness uses the unrestricted local proxy, never enables tracing and
   runs no UI thread, so these abort if reached. */
#define UNREACHABLE(name, symbol) \
  void name(void) __asm__(symbol); \
  void name(void) { abort(); }

UNREACHABLE(mojo_close, "MojoClose")
UNREACHABLE(endpoint_client, "_ZN4mojo23InterfaceEndpointClientC1ENS_29ScopedInterfaceEndpointHandleEPNS_34MessageReceiverWithResponderStatusENSt4__Cr10unique_ptrINS_15MessageReceiverENS4_14default_deleteIS6_EEEEN4base4spanIKjLm18446744073709551615EPSC_EE13scoped_refptrINSA_19SequencedTaskRunnerEEjPKcPFPFjvERNS_7MessageEEPFSJ_SN_E")
UNREACHABLE(endpoint_handle_move, "_ZN4mojo29ScopedInterfaceEndpointHandleC1EOS0_")
UNREACHABLE(endpoint_handle_destroy, "_ZN4mojo29ScopedInterfaceEndpointHandleD1Ev")
UNREACHABLE(router_start, "_ZN4mojo8internal15MultiplexRouter14StartReceivingEv")
UNREACHABLE(router_close, "_ZN4mojo8internal15MultiplexRouter16CloseMessagePipeEv")
UNREACHABLE(router_create, "_ZN4mojo8internal15MultiplexRouter6CreateENS_16ScopedHandleBaseINS_17MessagePipeHandleEEENS1_6ConfigEb13scoped_refptrIN4base19SequencedTaskRunnerEEPKc")
UNREACHABLE(router_task_runner, "_ZN4mojo8internal44GetTaskRunnerToUseFromUserProvidedTaskRunnerE13scoped_refptrIN4base19SequencedTaskRunnerEE")
UNREACHABLE(group_release, "_ZNK4mojo25AssociatedGroupController7ReleaseEv")
UNREACHABLE(lock_release, "_ZN7storage5mojom13FileLockProxy7ReleaseEPN4base4File5ErrorE")
UNREACHABLE(lock_proxy, "_ZN7storage5mojom13FileLockProxyC1EPN4mojo28MessageReceiverWithResponderE")
UNREACHABLE(lock_method_info, "_ZN7storage5mojom8FileLock20MessageToMethodInfo_ERN4mojo7MessageE")
UNREACHABLE(lock_method_name, "_ZN7storage5mojom8FileLock20MessageToMethodName_ERN4mojo7MessageE")
UNREACHABLE(track_create, "_ZN8perfetto6protos3gen15TrackDescriptorC1Ev")
UNREACHABLE(track_destroy, "_ZN8perfetto6protos3gen15TrackDescriptorD1Ev")
UNREACHABLE(glib_pump, "_ZN4base15MessagePumpGlibC1Ev")
UNREACHABLE(track_serialize, "_ZNK8perfetto6protos3gen15TrackDescriptor17SerializeAsStringEv")

void* const lock_validator_vtable[8] __asm__("_ZTVN7storage5mojom25FileLockResponseValidatorE") = {0};
