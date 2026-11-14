"""macOS discovery through the system Bonjour daemon rather than raw UDP sockets."""
import asyncio
import ctypes as C
from ipaddress import IPv4Address
import select
import socket
import time
import logging
_LOGGER = logging.getLogger(__name__)

from pyatv.core import mdns
from pyatv.core.scan import BaseScanner
from pyatv.protocols import PROTOCOLS
from pyatv.support.collections import CaseInsensitiveDict


def discover_services(service_types, timeout):
    lib = C.CDLL('/usr/lib/libSystem.B.dylib')
    ref_type = C.c_void_p
    u32, i32, text = C.c_uint32, C.c_int32, C.c_char_p
    browse_type = C.CFUNCTYPE(None, ref_type, u32, u32, i32, text, text, text, ref_type)
    resolve_type = C.CFUNCTYPE(None, ref_type, u32, u32, i32, text, text, C.c_uint16, C.c_uint16, ref_type, ref_type)
    query_type = C.CFUNCTYPE(None, ref_type, u32, u32, i32, text, C.c_uint16, C.c_uint16, C.c_uint16, ref_type, u32, ref_type)
    lib.DNSServiceBrowse.argtypes = [C.POINTER(ref_type), u32, u32, text, text, browse_type, ref_type]
    lib.DNSServiceResolve.argtypes = [C.POINTER(ref_type), u32, u32, text, text, text, resolve_type, ref_type]
    lib.DNSServiceQueryRecord.argtypes = [C.POINTER(ref_type), u32, u32, text, C.c_uint16, C.c_uint16, query_type, ref_type]
    for name in ('DNSServiceBrowse', 'DNSServiceResolve', 'DNSServiceQueryRecord', 'DNSServiceProcessResult', 'DNSServiceRefSockFD'):
        getattr(lib, name).restype = i32
    lib.DNSServiceProcessResult.argtypes = [ref_type]
    lib.DNSServiceRefSockFD.argtypes = [ref_type]
    lib.DNSServiceRefDeallocate.argtypes = [ref_type]
    refs, callbacks, pending, records, errors = [], [], set(), [], []

    def register(function, callback, *args):
        ref = ref_type()
        code = function(C.byref(ref), *args, callback, None)
        if code:
            errors.append(code)
        else:
            refs.append(ref)
            callbacks.append(callback)  # C callback pointers must remain alive until deallocation.

    def browse_callback(ref, flags, interface, error, name, kind, domain, context):
        _LOGGER.debug("browse: %r %r interface=%s error=%s flags=%s", name, kind, interface, error, flags)
        if error:
            errors.append(error)
            return
        if not flags & 2 or (interface, name, kind, domain) in pending:
            return
        pending.add((interface, name, kind, domain))

        def resolved(ref, flags, interface, error, fullname, hostname, port, length, data, context):
            if error:
                errors.append(error)
                return
            _LOGGER.debug("resolve: %r %r port=%s", name, hostname, socket.ntohs(port))
            raw = C.string_at(data, length)
            properties, offset = CaseInsensitiveDict(), 0
            while offset < len(raw):
                size = raw[offset]
                item = raw[offset + 1:offset + 1 + size]
                offset += size + 1
                key, _, value = item.partition(b'=')
                if key:
                    properties[key.decode('utf-8', 'replace')] = mdns.decode_value(value)

            def addressed(ref, flags, interface, error, fullname, rrtype, rrclass, length, data, ttl, context):
                _LOGGER.debug("address: %r type=%s error=%s flags=%s", fullname, rrtype, error, flags)
                if error:
                    errors.append(error)
                elif flags & 2 and rrtype == 1 and length == 4:
                    address = IPv4Address(C.string_at(data, 4))
                    service_type = kind.decode().rstrip('.') + '.local'
                    record = mdns.Service(service_type, name.decode(), address, socket.ntohs(port), properties)
                    if record not in records:
                        records.append(record)

            register(lib.DNSServiceQueryRecord, query_type(addressed), 0, interface, hostname, 1, 1)

        register(lib.DNSServiceResolve, resolve_type(resolved), 0, interface, name, kind, domain)

    try:
        for service_type in service_types:
            kind = service_type.removesuffix('.local').encode()
            register(lib.DNSServiceBrowse, browse_type(browse_callback), 0, 0, kind, b'local.')
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            by_fd = {lib.DNSServiceRefSockFD(ref): ref for ref in refs}
            by_fd = {fd: ref for fd, ref in by_fd.items() if fd >= 0}
            if not by_fd:
                break
            readable, _, _ = select.select(list(by_fd), [], [], min(.25, max(0, deadline - time.monotonic())))
            for fd in readable:
                code = lib.DNSServiceProcessResult(by_fd[fd])
                if code:
                    errors.append(code)
            if -65570 in errors:  # kDNSServiceErr_PolicyDenied
                raise PermissionError('Local network access is blocked. Enable ATV Remote Desktop in System Settings > Privacy & Security > Local Network, then restart the app.')
        if not records and errors:
            raise OSError(f'Bonjour discovery failed (DNS-SD error {errors[0]}).')
        return records
    finally:
        for ref in refs:
            lib.DNSServiceRefDeallocate(ref)


class NativeBonjourScanner(BaseScanner):
    async def process(self, timeout):
        services = await asyncio.to_thread(discover_services, self.services, timeout)
        # Group each host's services so protocol handlers see the complete response.
        for address in {service.address for service in services}:
            group = [service for service in services if service.address == address]
            model = next((service.properties.get('model') for service in group if service.type == '_device-info._tcp.local'), None)
            self.handle_response(mdns.Response(group, False, model))


async def scan(timeout=6):
    scanner = NativeBonjourScanner()
    for protocol, methods in PROTOCOLS.items():
        scanner.add_service_info(protocol, methods.service_info)
        for service_type, handler in methods.scan().items():
            scanner.add_service(service_type, handler, methods.device_info)
    return list((await scanner.discover(timeout)).values())
