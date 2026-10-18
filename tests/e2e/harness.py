"""End-to-end harness: real Kodi (headless, software GL) driving the real
packaged add-on over JSON-RPC, against generated HLS fixtures and a fake
yt-dlp. No network, no human, no GPU.

Configuration comes from the environment (set by `nix run .#e2e`):
  YOK_ADDON  built add-on directory (.../share/kodi/addons/plugin.video.myyoutubers)
  YOK_KODI   Kodi binary            YOK_XVFB  Xvfb binary
  YOK_MESA   Mesa output (software GL)
  YOK_MASTER master playlist dir the add-on was built with
  YOK_PORT   playlist server port the add-on was built with
"""
import glob
import json
import os
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from functools import partial
from threading import Thread

ADDON_ID = "plugin.video.myyoutubers"
DURATION = 1500  # seconds of fixture video
VIDEO_ID = "e2eVideo001"
MEDIA_PORT = 18098
RPC_PORT = 18080


def free_display():
    for n in range(90, 200):
        if not os.path.exists("/tmp/.X11-unix/X%d" % n) and not os.path.exists("/tmp/.X%d-lock" % n):
            return n
    raise RuntimeError("no free X display")


def make_fixture(root, variant):
    """Video-only and audio-only VOD HLS playlists, like YouTube's."""
    out = os.path.join(root, variant)
    if os.path.exists(os.path.join(out, "audio.m3u8")):
        return
    os.makedirs(out)
    seg = ["-hls_time", "5", "-hls_playlist_type", "vod"]
    if variant == "fmp4":
        seg += ["-hls_segment_type", "fmp4"]
    ext = "m4s" if variant == "fmp4" else "ts"
    src = ("testsrc2=size=320x180:rate=25,"
           "drawtext=text='%{pts\\:hms}':fontsize=40:fontcolor=white:x=10:y=10")
    run = lambda *a: subprocess.run(["ffmpeg", "-v", "error", "-y", *a], cwd=out, check=True)
    run("-f", "lavfi", "-i", src, "-t", str(DURATION), "-c:v", "libx264", "-preset", "ultrafast",
        "-g", "125", "-keyint_min", "125", "-sc_threshold", "0", "-pix_fmt", "yuv420p", "-an",
        "-f", "hls", *seg, "-hls_segment_filename", "v%04d." + ext,
        *(["-hls_fmp4_init_filename", "vinit.mp4"] if variant == "fmp4" else []), "video.m3u8")
    run("-f", "lavfi", "-i", "sine=frequency=440", "-t", str(DURATION), "-c:a", "aac", "-b:a", "48k",
        "-vn", "-f", "hls", *seg, "-hls_segment_filename", "a%04d." + ext,
        *(["-hls_fmp4_init_filename", "ainit.mp4"] if variant == "fmp4" else []), "audio.m3u8")


class Quiet(SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


_servers = []


def serve(directory, port):
    srv = ThreadingHTTPServer(("127.0.0.1", port), partial(Quiet, directory=directory))
    Thread(target=srv.serve_forever, daemon=True).start()
    _servers.append(srv)
    return srv


def stop_servers():
    """Close every server started with serve(), freeing its port for the next module."""
    while _servers:
        srv = _servers.pop()
        srv.shutdown()
        srv.server_close()


class Kodi:
    def __init__(self, workdir):
        self.dir = workdir
        self.home = os.path.join(workdir, "home")
        self.procs = []
        self.state = os.path.join(workdir, "state")
        os.makedirs(self.state)
        self.profile = os.path.join(self.home, ".kodi", "userdata", "addon_data", ADDON_ID)

    # -- lifecycle ---------------------------------------------------------
    def start(self, seed=None):
        """Start Xvfb and Kodi. `seed(profile_dir)` runs before Kodi launches."""
        e = os.environ
        userdata = os.path.join(self.home, ".kodi", "userdata")
        os.makedirs(userdata)
        os.makedirs(self.profile)
        if seed:
            seed(self.profile)
        addons = os.path.join(self.home, ".kodi", "addons")
        os.makedirs(addons)
        shutil.copytree(e["YOK_ADDON"], os.path.join(addons, ADDON_ID))
        with open(os.path.join(userdata, "guisettings.xml"), "w") as f:
            f.write('<settings version="2">'
                    '<setting id="services.webserver">true</setting>'
                    '<setting id="services.webserverport">%d</setting>'
                    '<setting id="services.webserverauthentication">false</setting>'
                    '<setting id="services.webserverusername"></setting>'
                    '<setting id="services.webserverpassword"></setting>'
                    '<setting id="audiooutput.audiodevice">ALSA:default</setting>'
                    '</settings>' % RPC_PORT)
        with open(os.path.join(userdata, "advancedsettings.xml"), "w") as f:
            f.write("<advancedsettings><loglevel>1</loglevel></advancedsettings>")
        with open(os.path.join(self.dir, "asound.conf"), "w") as f:
            f.write("pcm.!default { type null }\nctl.!default { type null }\n")
        os.makedirs(e["YOK_MASTER"], exist_ok=True)
        mesa = e["YOK_MESA"]
        env = dict(
            e, HOME=self.home, LIBGL_ALWAYS_SOFTWARE="1",
            LIBGL_DRIVERS_PATH=os.path.join(mesa, "lib", "dri"),
            LD_LIBRARY_PATH=os.path.join(mesa, "lib") + ":" + e.get("LD_LIBRARY_PATH", ""),
            ALSA_CONFIG_PATH=os.path.join(self.dir, "asound.conf"),
            YOK_STATE=self.state, YOK_MEDIA="http://127.0.0.1:%d" % MEDIA_PORT,
        )
        self.display = ":%d" % free_display()
        env["DISPLAY"] = self.display
        self.env = env
        self.procs.append(subprocess.Popen(
            [e["YOK_XVFB"], self.display, "-screen", "0", "1280x720x24", "+extension", "GLX", "-noreset"],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        self._wait(lambda: os.path.exists("/tmp/.X11-unix/X%s" % self.display[1:]), 30, "Xvfb")
        self._launch_kodi()

    def _launch_kodi(self):
        self.kodi_proc = subprocess.Popen(
            [os.environ["YOK_KODI"], "--standalone"], env=self.env,
            stdout=open(os.path.join(self.dir, "kodi.out"), "a"), stderr=subprocess.STDOUT)  # noqa: SIM115
        self.procs.append(self.kodi_proc)
        self._wait(self._rpc_up, 90, "Kodi JSON-RPC")
        self.rpc("Addons.SetAddonEnabled", addonid=ADDON_ID, enabled=True)
        self.rpc("Addons.SetAddonEnabled", addonid="inputstream.adaptive", enabled=True)

    def restart_kodi(self):
        """Quit Kodi (flushing its databases) and start it again, same profile."""
        self.rpc("Application.Quit")
        self.kodi_proc.wait(30)
        self.procs.remove(self.kodi_proc)
        self._launch_kodi()

    def video_db(self):
        (path,) = glob.glob(os.path.join(self.home, ".kodi", "userdata", "Database", "MyVideos*.db"))
        return path

    def make_tvshow_source(self, path):
        self.make_sources({path: "tvshows"})

    def make_sources(self, contents):
        """Set each path in `contents` to its content type ("tvshows" or
        "movies"), local information only.

        Kodi has no API for this; it is two columns of the video database's
        path table, so edit it while Kodi is stopped, then restart.
        """
        self.rpc("Application.Quit")
        self.kodi_proc.wait(30)
        self.procs.remove(self.kodi_proc)
        con = sqlite3.connect(self.video_db())
        try:
            for path, content in contents.items():
                path = path.rstrip("/") + "/"
                row = con.execute("SELECT idPath FROM path WHERE strPath=?", (path,)).fetchone()
                if row:
                    con.execute("UPDATE path SET strContent=?, strScraper='metadata.local' "
                                "WHERE idPath=?", (content, row[0]))
                else:
                    con.execute("INSERT INTO path (strPath, strContent, strScraper, scanRecursive, "
                                "useFolderNames, noUpdate, exclude) VALUES (?, ?, "
                                "'metadata.local', 0, 0, 0, 0)", (path, content))
            con.commit()
        finally:
            con.close()
        self._launch_kodi()

    def stop(self):
        for p in reversed(self.procs):
            p.terminate()
        for p in self.procs:
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()

    def log(self):
        path = os.path.join(self.home, ".kodi", "temp", "kodi.log")
        if not os.path.exists(path):
            return ""
        with open(path, errors="replace") as f:
            return f.read()

    # -- JSON-RPC ----------------------------------------------------------
    def rpc(self, method, **params):
        req = urllib.request.Request(
            "http://127.0.0.1:%d/jsonrpc" % RPC_PORT,
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
            {"content-type": "application/json"})
        r = json.load(urllib.request.urlopen(req, timeout=30))
        if "error" in r:
            raise RuntimeError("%s: %s" % (method, r["error"]))
        return r["result"]

    def _rpc_up(self):
        try:
            self.rpc("JSONRPC.Version")
            return True
        except Exception:
            return False

    @staticmethod
    def _wait(cond, timeout, what):
        end = time.time() + timeout
        while time.time() < end:
            if cond():
                return
            time.sleep(0.5)
        raise TimeoutError("timed out waiting for " + what)

    # -- player helpers ----------------------------------------------------
    def player(self):
        r = self.rpc("Player.GetActivePlayers")
        return r[0]["playerid"] if r else None

    def position(self):
        """Current playback position in seconds, or None if nothing plays."""
        pid = self.player()
        if pid is None:
            return None
        p = self.rpc("Player.GetProperties", playerid=pid, properties=["time", "speed"])
        t = p["time"]
        return t["hours"] * 3600 + t["minutes"] * 60 + t["seconds"] + t["milliseconds"] / 1000, p["speed"]

    def play(self, **params):
        url = "plugin://%s/?" % ADDON_ID + urllib.parse.urlencode(
            dict(mode="play", id=VIDEO_ID, title="E2E", **params))
        self.rpc("Player.Open", item={"file": url})

    def stop_playback(self):
        pid = self.player()
        if pid is not None:
            self.rpc("Player.Stop", playerid=pid)
        self._wait(lambda: self.player() is None, 15, "player to stop")

    def wait_position(self, near, tol=10, timeout=40):
        """Wait until playing at `near` (+/- tol seconds) and advancing."""
        last = None
        end = time.time() + timeout
        while time.time() < end:
            cur = self.position()
            if cur and cur[1] == 1:
                last = cur[0]
                if abs(last - near) <= tol:
                    return last
            time.sleep(0.5)
        raise AssertionError("expected playback near %ss, last position %r" % (near, last))

    def seek(self, seconds):
        h, r = divmod(int(seconds), 3600)
        m, s = divmod(r, 60)
        self.rpc("Player.Seek", playerid=self.player(),
                 value={"time": {"hours": h, "minutes": m, "seconds": s, "milliseconds": 0}})

    def set_engine(self, engine):
        """Playback engine setting: 'inputstream.adaptive' or 'ffmpeg'."""
        path = os.path.join(self.profile, "settings.xml")
        with open(path, "w") as f:
            f.write('<settings version="2"><setting id="playback_engine">%s</setting></settings>' % engine)

    def set_variant(self, name):
        with open(os.path.join(self.state, "variant"), "w") as f:
            f.write(name)

    def history(self):
        path = os.path.join(self.profile, "history.json")
        if not os.path.exists(path):
            return {}
        with open(path) as f:
            return json.load(f)

    def set_history(self, position):
        with open(os.path.join(self.profile, "history.json"), "w") as f:
            json.dump({VIDEO_ID: {"title": "E2E", "position": position, "duration": DURATION,
                                  "percentage": position * 100 // DURATION, "channel": "",
                                  "thumb": "", "date": "x"}}, f)
