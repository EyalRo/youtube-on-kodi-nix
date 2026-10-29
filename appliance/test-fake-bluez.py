"""A fake org.bluez on the system bus, for the Bluetooth VM test.

It implements what the UI uses: the ObjectManager at /, AgentManager1 at /org/bluez, one Adapter1
(hci0, powered off at first) and Device1 objects that appear when a scan starts. Every call is
appended to /tmp/fake-bluez.log so the test can check what the UI asked for.
"""
import asyncio

from dbus_fast import Variant
from dbus_fast.aio import MessageBus
from dbus_fast.constants import PropertyAccess
from dbus_fast.service import ServiceInterface, dbus_property, method

LOG = open("/tmp/fake-bluez.log", "a", buffering=1)
ADAPTER = "/org/bluez/hci0"


def log(*parts):
    print(*parts, file=LOG)


class Registry:
    """The exported objects (dbus-fast answers GetManagedObjects from their declared properties)."""

    def __init__(self, bus):
        self.bus = bus
        self.objects = {}  # path -> {interface: callable returning {prop: Variant}}

    def add(self, path, iface, interface_obj, props):
        self.bus.export(path, interface_obj)
        self.objects.setdefault(path, {})[iface] = props

    def remove(self, path, interface_obj):
        self.bus.unexport(path, interface_obj)
        self.objects.pop(path, None)



class AgentManager(ServiceInterface):
    def __init__(self):
        super().__init__("org.bluez.AgentManager1")

    @method()
    def RegisterAgent(self, agent: "o", capability: "s"):
        log("RegisterAgent", agent, capability)

    @method()
    def RequestDefaultAgent(self, agent: "o"):
        log("RequestDefaultAgent", agent)

    @method()
    def UnregisterAgent(self, agent: "o"):
        log("UnregisterAgent", agent)


class Device(ServiceInterface):
    def __init__(self, registry, address, name, cls=None, appearance=None, icon=None):
        super().__init__("org.bluez.Device1")
        self.registry = registry
        self.address = address
        self.dev_name = name
        self.cls = cls
        self.appearance = appearance
        self.icon = icon
        self.paired = False
        self.connected = False
        self.trusted = False
        self.path = ADAPTER + "/dev_" + address.replace(":", "_")

    def props(self):
        p = {
            "Address": Variant("s", self.address),
            "Name": Variant("s", self.dev_name),
            "Alias": Variant("s", self.dev_name),
            "Paired": Variant("b", self.paired),
            "Connected": Variant("b", self.connected),
            "Trusted": Variant("b", self.trusted),
        }
        if self.cls is not None:
            p["Class"] = Variant("u", self.cls)
        if self.appearance is not None:
            p["Appearance"] = Variant("q", self.appearance)
        if self.icon is not None:
            p["Icon"] = Variant("s", self.icon)
        return p

    @dbus_property(access=PropertyAccess.READ)
    def Name(self) -> "s":
        return self.dev_name

    @dbus_property(access=PropertyAccess.READ)
    def Alias(self) -> "s":
        return self.dev_name

    @dbus_property(access=PropertyAccess.READ)
    def Class(self) -> "u":
        return self.cls or 0

    @dbus_property(access=PropertyAccess.READ)
    def Appearance(self) -> "q":
        return self.appearance or 0

    @dbus_property(access=PropertyAccess.READ)
    def Icon(self) -> "s":
        return self.icon or ""

    @dbus_property(access=PropertyAccess.READ)
    def Address(self) -> "s":
        return self.address

    @dbus_property(access=PropertyAccess.READ)
    def Paired(self) -> "b":
        return self.paired

    @dbus_property(access=PropertyAccess.READ)
    def Connected(self) -> "b":
        return self.connected

    @dbus_property()
    def Trusted(self) -> "b":
        return self.trusted

    @Trusted.setter
    def Trusted(self, value: "b"):
        self.trusted = value
        log("Trusted", self.address, value)

    @method()
    async def Pair(self):
        log("Pair", self.address)
        await asyncio.sleep(0.8)
        self.paired = True

    @method()
    def Connect(self):
        log("Connect", self.address)
        self.connected = True

    @method()
    def Disconnect(self):
        log("Disconnect", self.address)
        self.connected = False


class Adapter(ServiceInterface):
    def __init__(self, registry):
        super().__init__("org.bluez.Adapter1")
        self.registry = registry
        self.powered = False
        self.discovering = False
        self.devices = {}

    def props(self):
        return {
            "Address": Variant("s", "00:11:22:33:44:55"),
            "Powered": Variant("b", self.powered),
            "Discovering": Variant("b", self.discovering),
        }

    @dbus_property()
    def Powered(self) -> "b":
        return self.powered

    @Powered.setter
    def Powered(self, value: "b"):
        self.powered = value
        log("Powered", value)

    @dbus_property(access=PropertyAccess.READ)
    def Discovering(self) -> "b":
        return self.discovering

    @method()
    async def StartDiscovery(self):
        log("StartDiscovery")
        self.discovering = True
        asyncio.get_running_loop().call_later(1.0, self.found)

    @method()
    def StopDiscovery(self):
        log("StopDiscovery")
        self.discovering = False

    @method()
    def RemoveDevice(self, device: "o"):
        log("RemoveDevice", device)
        dev = self.devices.pop(device, None)
        if dev is not None:
            self.registry.remove(device, dev)

    def found(self):
        # What a scan finds: both known controllers advertising their names, plus something else.
        for args in [
            ("E4:17:D8:00:00:02", "8BitDo M30 gamepad", 0x0508),
            ("E4:17:D8:00:00:01", "8BitDo Lite 2", None),
            ("00:1A:7D:DA:71:13", "Some Headphones", 0x240404),
        ]:
            dev = Device(self.registry, *args)
            if dev.path not in self.devices:
                self.devices[dev.path] = dev
                self.registry.add(dev.path, "org.bluez.Device1", dev, dev.props)


async def main():
    bus = await MessageBus(bus_type=__import__("dbus_fast").BusType.SYSTEM).connect()
    registry = Registry(bus)
    adapter = Adapter(registry)
    bus.export("/org/bluez", AgentManager())
    registry.add(ADAPTER, "org.bluez.Adapter1", adapter, adapter.props)
    await bus.request_name("org.bluez")
    log("ready")
    await asyncio.get_running_loop().create_future()


asyncio.run(main())
