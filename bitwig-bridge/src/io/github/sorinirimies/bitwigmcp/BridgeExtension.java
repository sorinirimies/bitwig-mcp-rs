package io.github.sorinirimies.bitwigmcp;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

import com.bitwig.extension.controller.ControllerExtension;
import com.bitwig.extension.controller.api.Action;
import com.bitwig.extension.controller.api.Application;
import com.bitwig.extension.controller.api.BooleanValue;
import com.bitwig.extension.controller.api.BrowserResultsItem;
import com.bitwig.extension.controller.api.BrowserResultsItemBank;
import com.bitwig.extension.controller.api.ClipLauncherSlot;
import com.bitwig.extension.controller.api.ClipLauncherSlotBank;
import com.bitwig.extension.controller.api.ControllerHost;
import com.bitwig.extension.controller.api.CursorRemoteControlsPage;
import com.bitwig.extension.controller.api.CursorTrack;
import com.bitwig.extension.controller.api.Device;
import com.bitwig.extension.controller.api.DeviceBank;
import com.bitwig.extension.controller.api.InsertionPoint;
import com.bitwig.extension.controller.api.IntegerValue;
import com.bitwig.extension.controller.api.MasterTrack;
import com.bitwig.extension.controller.api.NoteStep;
import com.bitwig.extension.controller.api.Parameter;
import com.bitwig.extension.controller.api.PinnableCursorClip;
import com.bitwig.extension.controller.api.PinnableCursorDevice;
import com.bitwig.extension.controller.api.PopupBrowser;
import com.bitwig.extension.controller.api.Project;
import com.bitwig.extension.controller.api.Scene;
import com.bitwig.extension.controller.api.SceneBank;
import com.bitwig.extension.controller.api.Send;
import com.bitwig.extension.controller.api.SendBank;
import com.bitwig.extension.controller.api.SettableColorValue;
import com.bitwig.extension.controller.api.SettableRangedValue;
import com.bitwig.extension.controller.api.Track;
import com.bitwig.extension.controller.api.TrackBank;
import com.bitwig.extension.controller.api.Transport;

/**
 * The bridge: a TCP server inside Bitwig that executes the commands of bitwig-mcp-rs.
 *
 * Deliberately dumb: each command maps to Controller API calls. Name lookup, dB and grid math
 * live in Rust. The protocol is documented in protocol.schema.json (generated from
 * crates/bitwig-mcp-rs/src/bridge/protocol.rs).
 *
 * Bitwig only lets an extension read values it subscribed to in init(), and only through banks
 * of a fixed size, hence the sizes below.
 */
public class BridgeExtension extends ControllerExtension
{
   static final int TRACKS = 64;
   static final int FX_TRACKS = 16;
   static final int SCENES = 64;
   static final int SENDS = 16;
   static final int DEVICES = 16;
   static final int PARAMS = 8;
   static final int RESULTS = 64;
   static final int GRID_WIDTH = 4096;
   static final int GRID_KEYS = 128;
   static final int SETTLE_MS = 150;
   static final int DEFAULT_PORT = 8417;

   private ControllerHost host;
   private Application app;
   private Transport transport;
   private Project project;
   private TrackBank mainBank;
   private TrackBank fxBank;
   private MasterTrack master;
   private SceneBank sceneBank;
   private CursorTrack cursorTrack;
   private PinnableCursorDevice cursorDevice;
   private CursorRemoteControlsPage remotePage;
   /** Pads of the cursor device when it is a drum machine: index = MIDI key. */
   private com.bitwig.extension.controller.api.DrumPadBank drumPads;
   private PinnableCursorClip cursorClip;
   private IntegerValue cursorClipSlot;
   private PopupBrowser browser;
   private BrowserResultsItemBank results;
   /** "Any" items of the browser's filter columns: selecting them resets the filters. */
   private final List<com.bitwig.extension.controller.api.BrowserFilterItem> wildcards = new ArrayList<>();
   private com.bitwig.extension.controller.api.MidiOut sysexOut;
   /** Send and clip-slot banks by "bank:index"; effect tracks and the master have none. */
   private final Map<String, SendBank> sendBanks = new HashMap<>();
   private final Map<String, ClipLauncherSlotBank> slotBanks = new HashMap<>();
   private final boolean withSysex;
   private Server server;
   /** The song plan, saved in the project (Studio I/O panel → MCP). */
   private com.bitwig.extension.controller.api.SettableStringValue songPlan;
   private volatile boolean running;
   private DeviceBank[] mainDevices;
   private DeviceBank[] fxDevices;
   private DeviceBank masterDevices;
   private BooleanValue[] mainSelected;
   private BooleanValue[] fxSelected;
   private BooleanValue masterSelected;
   /** Notes of the cursor clip by x * GRID_KEYS + key, as reported by Bitwig. */
   private final Map<Integer, NoteStep> notes = new HashMap<>();
   private double gridStep = 0.25;

   protected BridgeExtension(final BridgeExtensionDefinition definition, final ControllerHost host)
   {
      super(definition, host);
      this.withSysex = definition.getNumMidiOutPorts() > 0;
   }

   // ================================================================ setup

   @Override
   public void init()
   {
      host = getHost();
      app = host.createApplication();
      transport = host.createTransport();
      project = host.getProject();

      mark(app.projectName(), app.hasActiveEngine(), app.panelLayout(), app.canUndo(),
         app.canRedo(), project.isModified());
      mark(transport.isPlaying(), transport.isArrangerRecordEnabled(),
         transport.isArrangerLoopEnabled(), transport.arrangerLoopStart(),
         transport.arrangerLoopDuration(), transport.isMetronomeEnabled(),
         transport.tempo().value(), transport.tempo().value().displayedValue(),
         transport.playPosition(), transport.timeSignature(), transport.isPunchInEnabled(),
         transport.isPunchOutEnabled(), transport.isArrangerOverdubEnabled(),
         transport.isClipLauncherOverdubEnabled(), transport.isArrangerAutomationWriteEnabled(),
         transport.automationWriteMode(), transport.preRoll(),
         transport.defaultLaunchQuantization());

      cursorTrack = host.createCursorTrack("mcp-bridge", "MCP bridge", SENDS, SCENES, true);
      mark(cursorTrack.exists(), cursorTrack.name());

      mainBank = host.createTrackBank(TRACKS, SENDS, SCENES, true);
      fxBank = host.createEffectTrackBank(FX_TRACKS, SCENES);
      master = host.createMasterTrack(SCENES);
      mark(mainBank.itemCount(), fxBank.itemCount());

      mainDevices = new DeviceBank[TRACKS];
      mainSelected = new BooleanValue[TRACKS];
      for (int i = 0; i < TRACKS; i++)
      {
         final Track t = mainBank.getItemAt(i);
         observeTrack("main:" + i, t);
         mainDevices[i] = observeDevices(t);
         mainSelected[i] = cursorTrack.createEqualsValue(t);
         mark(mainSelected[i]);
      }
      fxDevices = new DeviceBank[FX_TRACKS];
      fxSelected = new BooleanValue[FX_TRACKS];
      for (int i = 0; i < FX_TRACKS; i++)
      {
         final Track t = fxBank.getItemAt(i);
         observeTrack("effect:" + i, t);
         fxDevices[i] = observeDevices(t);
         fxSelected[i] = cursorTrack.createEqualsValue(t);
         mark(fxSelected[i]);
      }
      observeTrack("master:0", master);
      masterDevices = observeDevices(master);
      masterSelected = cursorTrack.createEqualsValue(master);
      mark(masterSelected);

      sceneBank = mainBank.sceneBank();
      mark(sceneBank.itemCount());
      for (int i = 0; i < SCENES; i++)
      {
         final Scene s = sceneBank.getItemAt(i);
         mark(s.exists(), s.name(), s.color(), s.clipCount());
      }

      cursorDevice = cursorTrack.createCursorDevice();
      mark(cursorDevice.exists(), cursorDevice.name(), cursorDevice.position(),
         cursorDevice.isEnabled(), cursorDevice.isWindowOpen(), cursorDevice.presetName(),
         cursorDevice.isPlugin());
      drumPads = cursorDevice.createDrumPadBank(128);
      mark(cursorDevice.hasDrumPads(), drumPads.scrollPosition());
      for (int i = 0; i < 128; i++)
         mark(drumPads.getItemAt(i).exists(), drumPads.getItemAt(i).name());
      remotePage = cursorDevice.createCursorRemoteControlsPage(PARAMS);
      mark(remotePage.pageNames(), remotePage.selectedPageIndex(), remotePage.pageCount());
      for (int i = 0; i < PARAMS; i++)
      {
         final Parameter p = remotePage.getParameter(i);
         mark(p.exists(), p.name(), p.value(), p.value().displayedValue());
      }

      cursorClip = cursorTrack.createLauncherCursorClip(GRID_WIDTH, GRID_KEYS);
      mark(cursorClip.exists(), cursorClip.getLoopLength(), cursorClip.isLoopEnabled());
      try
      {
         cursorClipSlot = cursorClip.clipLauncherSlot().sceneIndex();
         mark(cursorClipSlot);
      }
      catch (final Throwable e)
      {
         cursorClipSlot = null; // older Bitwig: fall back to timing
      }
      cursorClip.scrollToKey(0);
      cursorClip.scrollToStep(0);
      cursorClip.setStepSize(gridStep);
      cursorClip.addNoteStepObserver(step -> {
         final int key = step.x() * GRID_KEYS + step.y();
         if (step.state() == NoteStep.State.NoteOn)
            notes.put(key, step);
         else
            notes.remove(key);
      });

      songPlan = host.getDocumentState().getStringSetting("Song plan", "MCP", 60000, "");
      songPlan.markInterested();
      browser = host.createPopupBrowser();
      sysexOut = withSysex ? host.getMidiOutPort(0) : null;
      mark(browser.exists(), browser.contentTypeNames(), browser.selectedContentTypeIndex());
      for (final com.bitwig.extension.controller.api.BrowserFilterColumn c : new com.bitwig.extension.controller.api.BrowserFilterColumn[] {
         browser.smartCollectionColumn(), browser.locationColumn(), browser.deviceColumn(),
         browser.categoryColumn(), browser.tagColumn(), browser.deviceTypeColumn(),
         browser.fileTypeColumn(), browser.creatorColumn() })
      {
         final com.bitwig.extension.controller.api.BrowserFilterItem any = c.getWildcardItem();
         any.isSelected().markInterested();
         wildcards.add(any);
      }
      results = browser.resultsColumn().createItemBank(RESULTS);
      mark(results.itemCount(), results.scrollPosition(), results.canScrollForwards());
      for (int i = 0; i < RESULTS; i++)
      {
         final BrowserResultsItem it = results.getItemAt(i);
         mark(it.exists(), it.name(), it.isSelected());
      }

      final int port = (int) Math.round(host.getPreferences()
         .getNumberSetting("TCP port (restart Bitwig after changing)", "Connection", 1024, 65535,
            1, "", DEFAULT_PORT)
         .getRaw());
      try
      {
         server = new Server(port, msg -> host.errorln("bitwig-mcp-rs bridge: " + msg));
      }
      catch (final java.io.IOException e)
      {
         host.errorln("bitwig-mcp-rs bridge: cannot listen on 127.0.0.1:" + port + ": " + e);
         host.showPopupNotification("MCP bridge: port " + port + " is in use");
         return;
      }
      running = true;
      pump();
      host.println("bitwig-mcp-rs bridge " + BridgeExtensionDefinition.VERSION
         + " listening on 127.0.0.1:" + server.port());
      host.showPopupNotification("MCP bridge on port " + server.port());
   }

   private static void mark(final Object... values)
   {
      for (final Object v : values)
      {
         if (v instanceof com.bitwig.extension.controller.api.Value)
            ((com.bitwig.extension.controller.api.Value<?>) v).markInterested();
      }
   }

   /** Bitwig throws for features a track kind lacks (sends on effect tracks, ...). */
   private static <T> T optional(final java.util.function.Supplier<T> get)
   {
      try
      {
         return get.get();
      }
      catch (final RuntimeException e)
      {
         return null;
      }
   }

   private void observeTrack(final String key, final Track t)
   {
      mark(t.exists(), t.name(), t.trackType(), t.isGroup(), t.isActivated(), t.color(),
         t.volume().value(), t.volume().value().displayedValue(), t.pan().value(),
         t.pan().value().displayedValue(), t.mute(), t.solo(), t.position());
      mark(optional(t::arm), optional(t::monitorMode));
      final SendBank sends = optional(t::sendBank);
      if (sends != null)
      {
         sendBanks.put(key, sends);
         for (int i = 0; i < sends.getSizeOfBank(); i++)
         {
            final Send s = sends.getItemAt(i);
            mark(s.exists(), s.name(), s.value(), s.value().displayedValue());
         }
      }
      final ClipLauncherSlotBank slots = optional(t::clipLauncherSlotBank);
      if (slots != null)
      {
         slotBanks.put(key, slots);
         for (int i = 0; i < slots.getSizeOfBank(); i++)
         {
            final ClipLauncherSlot c = slots.getItemAt(i);
            mark(c.exists(), c.hasContent(), c.name(), c.color(), c.isPlaying(), c.isRecording(),
               c.isPlaybackQueued());
         }
      }
   }

   private DeviceBank observeDevices(final Track t)
   {
      final DeviceBank bank = t.createDeviceBank(DEVICES);
      for (int i = 0; i < DEVICES; i++)
      {
         final Device d = bank.getItemAt(i);
         mark(d.exists(), d.name(), d.isEnabled(), d.isPlugin(), d.presetName());
      }
      return bank;
   }

   @Override
   public void exit()
   {
      running = false;
      if (server != null)
         server.close();
      host.println("bitwig-mcp-rs bridge stopped");
   }

   @Override
   public void flush()
   {
      // Nothing to push to hardware.
   }

   // ================================================================ transport layer

   /** Handles queued frames on the control thread, then runs again in a few milliseconds. */
   private void pump()
   {
      if (!running)
         return;
      Server.Incoming in;
      while ((in = server.poll()) != null)
         onFrame(in.conn, in.data);
      host.scheduleTask(this::pump, 5);
   }

   /** One reply, sent at most once. */
   final class Reply
   {
      private final Server.Conn conn;
      private final Object id;
      private boolean sent;

      Reply(final Server.Conn conn, final Object id)
      {
         this.conn = conn;
         this.id = id;
      }

      void ok(final Object result)
      {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("id", id);
         m.put("ok", true);
         m.put("result", result);
         send(m);
      }

      void fail(final String error)
      {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("id", id);
         m.put("ok", false);
         m.put("error", error);
         send(m);
      }

      private void send(final Map<String, Object> m)
      {
         if (sent)
            return;
         sent = true;
         try
         {
            conn.send(Json.write(m).getBytes(StandardCharsets.UTF_8));
         }
         catch (final Exception e)
         {
            host.errorln("bitwig-mcp-rs bridge: send failed: " + e);
         }
      }
   }

   private void onFrame(final Server.Conn conn, final byte[] data)
   {
      Reply reply = new Reply(conn, null);
      try
      {
         final Json.Obj req = new Json.Obj(Json.parse(new String(data, StandardCharsets.UTF_8)));
         reply = new Reply(conn, req.raw("id"));
         dispatch(req.str("cmd"), req, reply);
      }
      catch (final Throwable e)
      {
         reply.fail(e.getMessage() == null ? e.toString() : e.getMessage());
      }
   }

   /** Runs {@code r} later on the control thread; failures go to the reply. */
   private void later(final int ms, final Reply reply, final Runnable r)
   {
      host.scheduleTask(() -> {
         try
         {
            r.run();
         }
         catch (final Throwable e)
         {
            reply.fail(e.getMessage() == null ? e.toString() : e.getMessage());
         }
      }, ms);
   }

   interface Cond
   {
      boolean ok();
   }

   /** Polls {@code c} every 25 ms; runs {@code then} when true, fails after the timeout. */
   private void waitFor(final Cond c, final int timeoutMs, final String what, final Reply reply,
      final Runnable then)
   {
      final long deadline = System.currentTimeMillis() + timeoutMs;
      final Runnable[] poll = new Runnable[1];
      poll[0] = () -> {
         if (c.ok())
            then.run();
         else if (System.currentTimeMillis() > deadline)
            reply.fail("timed out waiting for " + what);
         else
            later(25, reply, poll[0]);
      };
      later(0, reply, poll[0]);
   }

   // ================================================================ dispatch

   private void dispatch(final String cmd, final Json.Obj q, final Reply r)
   {
      switch (cmd)
      {
         case "hello":
         {
            final Map<String, Object> m = new LinkedHashMap<>();
            m.put("protocol", BridgeExtensionDefinition.PROTOCOL);
            m.put("extension_version", BridgeExtensionDefinition.VERSION);
            m.put("api_version", host.getHostApiVersion());
            m.put("bitwig_version", host.getHostVersion());
            r.ok(m);
            return;
         }
         case "get_project":
            r.ok(projectSnapshot());
            return;
         case "transport":
            transportAction(q.str("action"));
            later(SETTLE_MS, r, () -> r.ok(transportSnapshot()));
            return;
         case "set_transport":
            setTransport(q);
            later(SETTLE_MS, r, () -> r.ok(transportSnapshot()));
            return;
         case "create_track":
            createTrack(q, r);
            return;
         case "set_track":
         {
            final Addr a = addr(q.obj("track"));
            setTrack(a, q);
            later(SETTLE_MS, r, () -> r.ok(trackSnapshot(a)));
            return;
         }
         case "track_action":
            trackAction(addr(q.obj("track")), q.str("action"), r);
            return;
         case "create_scene":
         {
            final int before = sceneBank.itemCount().get();
            project.createScene();
            waitFor(() -> sceneBank.itemCount().get() > before, 3000, "the new scene", r, () -> {
               final Map<String, Object> m = new LinkedHashMap<>();
               m.put("index", before);
               r.ok(m);
            });
            return;
         }
         case "set_scene":
         {
            final Scene s = scene(q.integer("scene"));
            if (q.has("name"))
               s.name().set(q.str("name"));
            if (q.has("color"))
               setColor(s.color(), q.list("color"));
            later(SETTLE_MS, r, () -> r.ok(sceneSnapshot(q.integer("scene"))));
            return;
         }
         case "scene_action":
            sceneAction(q.integer("scene"), q.str("action"), r);
            return;
         case "create_clip":
            createClip(q, r);
            return;
         case "clip_action":
            clipAction(q, r);
            return;
         case "set_clip":
            setClip(q, r);
            return;
         case "write_notes":
            writeNotes(q, r);
            return;
         case "read_notes":
            readNotes(q, r);
            return;
         case "insert_device":
            insertDevice(q, r);
            return;
         case "get_device":
            focusDevice(q, r, () -> r.ok(cursorDeviceSnapshot()));
            return;
         case "set_device_param":
            focusDevice(q, r, () -> {
               final int i = q.integer("param");
               if (i < 0 || i >= PARAMS)
                  throw new IllegalArgumentException("param must be 1-" + PARAMS);
               final Parameter p = remotePage.getParameter(i);
               if (!p.exists().get())
                  throw new IllegalArgumentException("parameter " + (i + 1) + " is empty on this page");
               p.value().setImmediately(clamp01(q.num("value")));
               later(SETTLE_MS, r, () -> r.ok(paramSnapshot(i)));
            });
            return;
         case "device_action":
            deviceAction(q, r);
            return;
         case "play_notes":
            playNotes(q, r);
            return;
         case "send_midi":
         {
            final Track t = addr(q.obj("track")).track;
            int n = 0;
            for (final Object o : q.list("messages"))
            {
               final List<?> m = (List<?>) o;
               t.sendMidi(intOf(m.get(0)), intOf(m.get(1)), intOf(m.get(2)));
               n++;
            }
            final Map<String, Object> res = new LinkedHashMap<>();
            res.put("sent", n);
            r.ok(res);
            return;
         }
         case "send_sysex":
         {
            final List<Object> raw = q.list("bytes");
            final byte[] bytes = new byte[raw.size()];
            for (int i = 0; i < bytes.length; i++)
               bytes[i] = (byte) intOf(raw.get(i));
            if (bytes.length < 3 || (bytes[0] & 0xFF) != 0xF0 || (bytes[bytes.length - 1] & 0xFF) != 0xF7)
               throw new IllegalArgumentException("SysEx must start with F0 and end with F7");
            if (sysexOut == null)
               throw new IllegalArgumentException("SysEx needs the \"MCP bridge + SysEx out\" "
                  + "controller (Bitwig: Settings > Controllers > bitwig-mcp-rs) with the synth's "
                  + "MIDI port as its output");
            sysexOut.sendSysex(bytes);
            final Map<String, Object> res = new LinkedHashMap<>();
            res.put("sent", bytes.length);
            res.put("note", "sent to the MIDI output assigned to the MCP bridge controller");
            r.ok(res);
            return;
         }
         case "get_song_plan":
         {
            final Map<String, Object> m = new LinkedHashMap<>();
            m.put("text", songPlan.get());
            r.ok(m);
            return;
         }
         case "set_song_plan":
         {
            final String text = q.str("text");
            if (text.length() > 60000)
               throw new IllegalArgumentException("the song plan is limited to 60000 characters");
            songPlan.set(text);
            later(SETTLE_MS, r, () -> {
               final Map<String, Object> m = new LinkedHashMap<>();
               m.put("stored", songPlan.get().length());
               r.ok(m);
            });
            return;
         }
         case "drum_pad_insert":
            focusDevice(q, r, () -> {
               if (!cursorDevice.hasDrumPads().get())
                  throw new IllegalArgumentException("this device has no drum pads (use Drum Machine)");
               final int key = q.integer("key");
               if (key < 0 || key > 127)
                  throw new IllegalArgumentException("key must be 0-127");
               // Bitwig scrolls a drum machine's pads to C1 (36): bring index 0 back to key 0
               drumPads.scrollPosition().set(0);
               waitFor(() -> drumPads.scrollPosition().get() == 0, 2000, "the drum pads", r,
                  () -> later(100, r, () -> insertPad(q, key, r)));
            });
            return;
         case "list_browser":
            listBrowser(addr(q.obj("track")), q.strOr("content_type", "Devices"),
               Math.max(1, Math.min(2000, q.integer("max"))), r);
            return;
         case "list_actions":
            r.ok(listActions(q.strOr("query", "")));
            return;
         case "invoke_action":
         {
            final Action a = app.getAction(q.str("id"));
            if (a == null)
               throw new IllegalArgumentException("no action with id '" + q.str("id") + "'");
            a.invoke();
            final Map<String, Object> m = new LinkedHashMap<>();
            m.put("invoked", a.getName());
            later(SETTLE_MS, r, () -> r.ok(m));
            return;
         }
         case "undo":
            app.undo();
            later(SETTLE_MS, r, () -> r.ok(appSnapshot()));
            return;
         case "redo":
            app.redo();
            later(SETTLE_MS, r, () -> r.ok(appSnapshot()));
            return;
         case "set_view":
            setView(q);
            later(SETTLE_MS, r, () -> r.ok(appSnapshot()));
            return;
         default:
            throw new IllegalArgumentException("unknown command '" + cmd + "'");
      }
   }

   // ================================================================ addressing

   /** A resolved track plus its device bank and "is cursor track" value. */
   static final class Addr
   {
      final String bank;
      final int index;
      final Track track;
      final DeviceBank devices;
      final BooleanValue selected;

      String key()
      {
         return bank + ":" + index;
      }

      Addr(final String bank, final int index, final Track track, final DeviceBank devices,
         final BooleanValue selected)
      {
         this.bank = bank;
         this.index = index;
         this.track = track;
         this.devices = devices;
         this.selected = selected;
      }
   }

   /**
    * The flat main bank also lists the effect tracks and the master; those are addressed
    * through their own banks, so "main" means audio, instrument and group tracks only.
    */
   private boolean isRegular(final int i)
   {
      if (i < 0 || i >= TRACKS || !mainBank.getItemAt(i).exists().get())
         return false;
      final String type = mainBank.getItemAt(i).trackType().get();
      return !"Effect".equals(type) && !"Master".equals(type);
   }

   private Addr addr(final Json.Obj a)
   {
      final String bank = a.str("bank");
      final int i = a.integer("index");
      switch (bank)
      {
         case "main":
            if (!isRegular(i))
               throw new IllegalArgumentException("no audio, instrument or group track at " + i);
            return new Addr(bank, i, mainBank.getItemAt(i), mainDevices[i], mainSelected[i]);
         case "effect":
            if (i < 0 || i >= FX_TRACKS || !fxBank.getItemAt(i).exists().get())
               throw new IllegalArgumentException("no effect track " + (i + 1));
            return new Addr(bank, i, fxBank.getItemAt(i), fxDevices[i], fxSelected[i]);
         case "master":
            return new Addr(bank, 0, master, masterDevices, masterSelected);
         default:
            throw new IllegalArgumentException("unknown track bank '" + bank + "'");
      }
   }

   private Scene scene(final int i)
   {
      if (i < 0 || i >= SCENES || !sceneBank.getItemAt(i).exists().get())
         throw new IllegalArgumentException("no scene " + (i + 1));
      return sceneBank.getItemAt(i);
   }

   private ClipLauncherSlotBank slots(final Addr a)
   {
      final ClipLauncherSlotBank b = slotBanks.get(a.key());
      if (b == null)
         throw new IllegalArgumentException("this track has no clip launcher slots");
      return b;
   }

   private ClipLauncherSlot slot(final Addr a, final int i)
   {
      if (i < 0 || i >= SCENES)
         throw new IllegalArgumentException("slot must be 1-" + SCENES);
      return slots(a).getItemAt(i);
   }

   private static Device device(final Addr a, final int i)
   {
      if (i < 0 || i >= DEVICES || !a.devices.getItemAt(i).exists().get())
         throw new IllegalArgumentException("no device " + (i + 1) + " on this track");
      return a.devices.getItemAt(i);
   }

   private static int intOf(final Object o)
   {
      return (int) Math.round(((Number) o).doubleValue());
   }

   private static double clamp01(final double v)
   {
      return Math.max(0, Math.min(1, v));
   }

   private static void setColor(final SettableColorValue c, final List<Object> rgb)
   {
      c.set(intOf(rgb.get(0)) / 255f, intOf(rgb.get(1)) / 255f, intOf(rgb.get(2)) / 255f);
   }

   // ================================================================ snapshots

   private static String hex(final com.bitwig.extension.controller.api.ColorValue c)
   {
      return String.format("#%02x%02x%02x", Math.round(c.red() * 255), Math.round(c.green() * 255),
         Math.round(c.blue() * 255));
   }

   private Map<String, Object> projectSnapshot()
   {
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("application", appSnapshot());
      m.put("transport", transportSnapshot());
      final List<Object> tracks = new ArrayList<>();
      for (int i = 0; i < TRACKS; i++)
         if (isRegular(i))
            tracks.add(trackSnapshot(new Addr("main", i, mainBank.getItemAt(i), mainDevices[i],
               mainSelected[i])));
      m.put("tracks", tracks);
      final List<Object> fx = new ArrayList<>();
      for (int i = 0; i < FX_TRACKS; i++)
         if (fxBank.getItemAt(i).exists().get())
            fx.add(trackSnapshot(new Addr("effect", i, fxBank.getItemAt(i), fxDevices[i],
               fxSelected[i])));
      m.put("effect_tracks", fx);
      m.put("master", trackSnapshot(new Addr("master", 0, master, masterDevices, masterSelected)));
      final List<Object> scenes = new ArrayList<>();
      for (int i = 0; i < SCENES; i++)
         if (sceneBank.getItemAt(i).exists().get())
            scenes.add(sceneSnapshot(i));
      m.put("scenes", scenes);
      if (mainBank.itemCount().get() > TRACKS)
         m.put("note", "only the first " + TRACKS + " of " + mainBank.itemCount().get()
            + " tracks are visible to the bridge");
      return m;
   }

   private Map<String, Object> appSnapshot()
   {
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("project_name", app.projectName().get());
      m.put("engine_active", app.hasActiveEngine().get());
      m.put("layout", app.panelLayout().get());
      m.put("can_undo", app.canUndo().get());
      m.put("can_redo", app.canRedo().get());
      m.put("modified", project.isModified().get());
      return m;
   }

   private Map<String, Object> transportSnapshot()
   {
      final Transport t = transport;
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("playing", t.isPlaying().get());
      m.put("recording", t.isArrangerRecordEnabled().get());
      m.put("tempo", t.tempo().value().getRaw());
      m.put("tempo_text", t.tempo().value().displayedValue().get());
      m.put("time_signature", t.timeSignature().get());
      m.put("position", t.playPosition().get());
      m.put("position_text", t.playPosition().getFormatted());
      m.put("loop", t.isArrangerLoopEnabled().get());
      m.put("loop_start", t.arrangerLoopStart().get());
      m.put("loop_length", t.arrangerLoopDuration().get());
      m.put("metronome", t.isMetronomeEnabled().get());
      m.put("punch_in", t.isPunchInEnabled().get());
      m.put("punch_out", t.isPunchOutEnabled().get());
      m.put("arranger_overdub", t.isArrangerOverdubEnabled().get());
      m.put("launcher_overdub", t.isClipLauncherOverdubEnabled().get());
      m.put("automation_write", t.isArrangerAutomationWriteEnabled().get());
      m.put("automation_mode", t.automationWriteMode().get());
      m.put("pre_roll", t.preRoll().get());
      m.put("launch_quantization", t.defaultLaunchQuantization().get());
      return m;
   }

   private Map<String, Object> trackSnapshot(final Addr a)
   {
      final Track t = a.track;
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("bank", a.bank);
      m.put("index", a.index);
      m.put("name", t.name().get());
      m.put("type", t.trackType().get());
      m.put("is_group", t.isGroup().get());
      m.put("selected", a.selected.get());
      m.put("activated", t.isActivated().get());
      m.put("color", hex(t.color()));
      m.put("volume", t.volume().value().get());
      m.put("volume_text", t.volume().value().displayedValue().get());
      m.put("pan", t.pan().value().get());
      m.put("pan_text", t.pan().value().displayedValue().get());
      m.put("mute", t.mute().get());
      m.put("solo", t.solo().get());
      m.put("arm", optional(() -> t.arm().get()));
      m.put("monitor_mode", optional(() -> t.monitorMode().get()));
      final List<Object> sends = new ArrayList<>();
      final SendBank sb = sendBanks.get(a.key());
      for (int i = 0; sb != null && i < sb.getSizeOfBank(); i++)
      {
         final Send s = sb.getItemAt(i);
         if (!s.exists().get())
            continue;
         final Map<String, Object> x = new LinkedHashMap<>();
         x.put("index", i);
         x.put("name", s.name().get());
         x.put("volume", s.value().get());
         x.put("volume_text", s.value().displayedValue().get());
         sends.add(x);
      }
      m.put("sends", sends);
      m.put("devices", devicesSnapshot(a));
      final List<Object> clips = new ArrayList<>();
      final ClipLauncherSlotBank slots = slotBanks.get(a.key());
      for (int i = 0; slots != null && i < slots.getSizeOfBank(); i++)
      {
         final ClipLauncherSlot c = slots.getItemAt(i);
         if (!c.hasContent().get())
            continue;
         final Map<String, Object> x = new LinkedHashMap<>();
         x.put("slot", i);
         x.put("name", c.name().get());
         x.put("color", hex(c.color()));
         x.put("playing", c.isPlaying().get());
         x.put("recording", c.isRecording().get());
         x.put("queued", c.isPlaybackQueued().get());
         clips.add(x);
      }
      m.put("clips", clips);
      return m;
   }

   private List<Object> devicesSnapshot(final Addr a)
   {
      final List<Object> list = new ArrayList<>();
      for (int i = 0; i < DEVICES; i++)
      {
         final Device d = a.devices.getItemAt(i);
         if (!d.exists().get())
            continue;
         final Map<String, Object> x = new LinkedHashMap<>();
         x.put("index", i);
         x.put("name", d.name().get());
         x.put("enabled", d.isEnabled().get());
         x.put("plugin", d.isPlugin().get());
         x.put("preset", d.presetName().get());
         list.add(x);
      }
      return list;
   }

   private Map<String, Object> sceneSnapshot(final int i)
   {
      final Scene s = sceneBank.getItemAt(i);
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("index", i);
      m.put("name", s.name().get());
      m.put("color", hex(s.color()));
      m.put("clip_count", s.clipCount().get());
      return m;
   }

   private Map<String, Object> cursorDeviceSnapshot()
   {
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("name", cursorDevice.name().get());
      m.put("enabled", cursorDevice.isEnabled().get());
      m.put("plugin", cursorDevice.isPlugin().get());
      m.put("preset", cursorDevice.presetName().get());
      m.put("window_open", cursorDevice.isWindowOpen().get());
      m.put("pages", remotePage.pageNames().get());
      m.put("page", remotePage.selectedPageIndex().get());
      final List<Object> params = new ArrayList<>();
      for (int i = 0; i < PARAMS; i++)
         if (remotePage.getParameter(i).exists().get())
            params.add(paramSnapshot(i));
      m.put("params", params);
      if (cursorDevice.hasDrumPads().get())
      {
         final List<Object> pads = new ArrayList<>();
         for (int i = 0; i < 128; i++)
         {
            final com.bitwig.extension.controller.api.DrumPad pad = drumPads.getItemAt(i);
            if (!pad.exists().get() || pad.name().get().isEmpty())
               continue;
            final Map<String, Object> x = new LinkedHashMap<>();
            x.put("key", drumPads.scrollPosition().get() + i);
            x.put("name", pad.name().get());
            pads.add(x);
         }
         m.put("drum_pads", pads);
      }
      return m;
   }

   private Map<String, Object> paramSnapshot(final int i)
   {
      final Parameter p = remotePage.getParameter(i);
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("index", i);
      m.put("name", p.name().get());
      m.put("value", p.value().get());
      m.put("display", p.value().displayedValue().get());
      return m;
   }

   private Map<String, Object> clipSnapshot()
   {
      final List<Object> list = new ArrayList<>();
      final List<Integer> keys = new ArrayList<>(notes.keySet());
      java.util.Collections.sort(keys);
      for (final int k : keys)
      {
         final NoteStep s = notes.get(k);
         final Map<String, Object> x = new LinkedHashMap<>();
         x.put("x", s.x());
         x.put("key", s.y());
         x.put("velocity", (int) Math.round(s.velocity() * 127));
         x.put("duration", s.duration());
         list.add(x);
      }
      final Map<String, Object> clip = new LinkedHashMap<>();
      clip.put("loop_length", cursorClip.getLoopLength().get());
      clip.put("loop", cursorClip.isLoopEnabled().get());
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("step", gridStep);
      m.put("clip", clip);
      m.put("notes", list);
      return m;
   }

   // ================================================================ commands

   private void transportAction(final String action)
   {
      switch (action)
      {
         case "play":
            transport.play();
            break;
         case "stop":
            transport.stop();
            break;
         case "toggle_play":
            transport.togglePlay();
            break;
         case "record":
            transport.record();
            break;
         case "restart":
            transport.restart();
            break;
         case "rewind":
            transport.rewind();
            break;
         case "fast_forward":
            transport.fastForward();
            break;
         case "tap_tempo":
            transport.tapTempo();
            break;
         case "stop_all_clips":
            for (int i = 0; i < TRACKS; i++)
               mainBank.getItemAt(i).stop();
            for (int i = 0; i < FX_TRACKS; i++)
               fxBank.getItemAt(i).stop();
            break;
         case "add_cue_marker":
            transport.addCueMarkerAtPlaybackPosition();
            break;
         case "next_cue_marker":
            transport.jumpToNextCueMarker();
            break;
         case "previous_cue_marker":
            transport.jumpToPreviousCueMarker();
            break;
         default:
            throw new IllegalArgumentException("unknown transport action '" + action + "'");
      }
   }

   private void setTransport(final Json.Obj q)
   {
      final Transport t = transport;
      if (q.has("tempo"))
         t.tempo().value().setRaw(q.num("tempo"));
      if (q.has("loop"))
         t.isArrangerLoopEnabled().set(q.bool("loop"));
      if (q.has("loop_start"))
         t.arrangerLoopStart().set(q.num("loop_start"));
      if (q.has("loop_length"))
         t.arrangerLoopDuration().set(q.num("loop_length"));
      if (q.has("metronome"))
         t.isMetronomeEnabled().set(q.bool("metronome"));
      if (q.has("position"))
         t.setPosition(q.num("position"));
      if (q.has("time_signature"))
         t.timeSignature().set(q.str("time_signature"));
      if (q.has("punch_in"))
         t.isPunchInEnabled().set(q.bool("punch_in"));
      if (q.has("punch_out"))
         t.isPunchOutEnabled().set(q.bool("punch_out"));
      if (q.has("arranger_overdub"))
         t.isArrangerOverdubEnabled().set(q.bool("arranger_overdub"));
      if (q.has("launcher_overdub"))
         t.isClipLauncherOverdubEnabled().set(q.bool("launcher_overdub"));
      if (q.has("automation_write"))
         t.isArrangerAutomationWriteEnabled().set(q.bool("automation_write"));
      if (q.has("automation_mode"))
         t.automationWriteMode().set(q.str("automation_mode"));
      if (q.has("pre_roll"))
         t.preRoll().set(q.str("pre_roll"));
      if (q.has("launch_quantization"))
         t.defaultLaunchQuantization().set(q.str("launch_quantization"));
   }

   private void createTrack(final Json.Obj q, final Reply r)
   {
      final String kind = q.str("kind");
      final int position = q.integer("position");
      if (kind.equals("effect"))
      {
         final int before = fxBank.itemCount().get();
         app.createEffectTrack(-1);
         waitFor(() -> fxBank.itemCount().get() > before, 3000, "the new track", r, () -> {
            final Map<String, Object> m = new LinkedHashMap<>();
            m.put("bank", "effect");
            m.put("index", before);
            r.ok(m);
         });
         return;
      }
      final int before = mainBank.itemCount().get();
      if (kind.equals("audio"))
         app.createAudioTrack(position);
      else if (kind.equals("instrument"))
         app.createInstrumentTrack(position);
      else
         throw new IllegalArgumentException("unknown track kind '" + kind + "'");
      // Bitwig selects a track it creates: find it through the selection, not by counting
      // (the flat bank also holds the effect tracks and the master at its end).
      waitFor(() -> mainBank.itemCount().get() > before, 3000, "the new track", r,
         () -> later(150, r, () -> {
            int index = -1;
            for (int i = 0; i < TRACKS; i++)
               if (isRegular(i) && mainSelected[i].get())
                  index = i;
            if (index < 0)
               throw new IllegalStateException("created a track but could not find it; "
                  + "check get_project before changing anything");
            final Map<String, Object> m = new LinkedHashMap<>();
            m.put("bank", "main");
            m.put("index", index);
            m.put("type", mainBank.getItemAt(index).trackType().get());
            r.ok(m);
         }));
   }

   private void setTrack(final Addr a, final Json.Obj q)
   {
      final Track t = a.track;
      if (q.has("name"))
         t.name().set(q.str("name"));
      if (q.has("color"))
         setColor(t.color(), q.list("color"));
      if (q.has("volume"))
         t.volume().value().setImmediately(clamp01(q.num("volume")));
      if (q.has("pan"))
         t.pan().value().setImmediately(clamp01(q.num("pan")));
      if (q.has("mute"))
         t.mute().set(q.bool("mute"));
      if (q.has("solo"))
         t.solo().set(q.bool("solo"));
      if (q.has("arm"))
         t.arm().set(q.bool("arm"));
      if (q.has("activated"))
         t.isActivated().set(q.bool("activated"));
      if (q.has("monitor_mode"))
         t.monitorMode().set(q.str("monitor_mode"));
      final SendBank sb = sendBanks.get(a.key());
      for (final Object o : q.list("sends"))
      {
         final Json.Obj s = new Json.Obj(o);
         final int i = s.integer("index");
         if (sb == null || i < 0 || i >= sb.getSizeOfBank() || !sb.getItemAt(i).exists().get())
            throw new IllegalArgumentException("no send " + (i + 1) + " on this track");
         sb.getItemAt(i).value().setImmediately(clamp01(s.num("volume")));
      }
   }

   private void trackAction(final Addr a, final String action, final Reply r)
   {
      final Track t = a.track;
      switch (action)
      {
         case "select":
            cursorTrack.selectChannel(t);
            break;
         case "duplicate":
            t.duplicate();
            break;
         case "delete":
            t.deleteObject();
            break;
         case "stop_clips":
            t.stop();
            break;
         case "return_to_arrangement":
            t.returnToArrangement();
            break;
         case "show":
            t.makeVisibleInArranger();
            t.makeVisibleInMixer();
            break;
         default:
            throw new IllegalArgumentException("unknown track action '" + action + "'");
      }
      later(SETTLE_MS, r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("done", action);
         m.put("track_count", mainBank.itemCount().get());
         r.ok(m);
      });
   }

   private void sceneAction(final int i, final String action, final Reply r)
   {
      final Scene s = scene(i);
      switch (action)
      {
         case "launch":
            s.launch();
            break;
         case "select":
            s.selectInEditor();
            break;
         case "duplicate":
            s.duplicateObject();
            break;
         case "delete":
            s.deleteObject();
            break;
         default:
            throw new IllegalArgumentException("unknown scene action '" + action + "'");
      }
      later(SETTLE_MS, r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("done", action);
         m.put("scene_count", sceneBank.itemCount().get());
         r.ok(m);
      });
   }

   private void createClip(final Json.Obj q, final Reply r)
   {
      final Addr a = addr(q.obj("track"));
      final int i = q.integer("slot");
      final ClipLauncherSlot s = slot(a, i);
      if (s.hasContent().get())
         throw new IllegalArgumentException("slot " + (i + 1) + " already holds a clip");
      slots(a).createEmptyClip(i, q.integer("length"));
      waitFor(() -> s.hasContent().get(), 3000, "the new clip", r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("slot", i);
         r.ok(m);
      });
   }

   private void clipAction(final Json.Obj q, final Reply r)
   {
      final Addr a = addr(q.obj("track"));
      final int i = q.integer("slot");
      final ClipLauncherSlot s = slot(a, i);
      final String action = q.str("action");
      switch (action)
      {
         case "launch":
            s.launch();
            break;
         case "stop":
            a.track.stop();
            break;
         case "select":
            s.select();
            break;
         case "record":
            s.record();
            break;
         case "delete":
            s.deleteObject();
            break;
         case "duplicate":
            s.duplicateClip();
            break;
         case "show":
            s.showInEditor();
            break;
         default:
            throw new IllegalArgumentException("unknown clip action '" + action + "'");
      }
      later(SETTLE_MS, r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("done", action);
         m.put("has_content", s.hasContent().get());
         m.put("playing", s.isPlaying().get());
         r.ok(m);
      });
   }

   /** Points the cursor clip at (track, slot), then runs {@code then}. */
   private void focusClip(final Json.Obj q, final Reply r, final Runnable then)
   {
      final Addr a = addr(q.obj("track"));
      final int i = q.integer("slot");
      final ClipLauncherSlot s = slot(a, i);
      if (!s.hasContent().get())
         throw new IllegalArgumentException("slot " + (i + 1) + " is empty");
      // The note map is never cleared: Bitwig reports only grid cells that change (new clip,
      // new step size), so a cell equal in both would be lost after a clear.
      cursorTrack.selectChannel(a.track);
      s.select();
      waitFor(() -> a.selected.get() && cursorClip.exists().get()
         && (cursorClipSlot == null || cursorClipSlot.get() == i), 3000, "the clip to focus", r,
         // a little extra so the note observers catch up with the new clip
         () -> later(cursorClipSlot == null ? 400 : 100, r, then));
   }

   private void setGrid(final double step)
   {
      if (step != gridStep)
      {
         gridStep = step;
         cursorClip.setStepSize(step);
      }
   }

   private void writeNotes(final Json.Obj q, final Reply r)
   {
      final double step = q.num("step");
      final boolean clear = q.bool("clear");
      final List<Object> list = q.list("notes");
      focusClip(q, r, () -> {
         setGrid(step);
         later(60, r, () -> {
            if (clear)
               cursorClip.clearSteps();
            later(clear ? 60 : 0, r, () -> {
               for (final Object o : list)
               {
                  final Json.Obj n = new Json.Obj(o);
                  final int x = n.integer("x");
                  final int key = n.integer("key");
                  if (x < 0 || x >= GRID_WIDTH || key < 0 || key >= GRID_KEYS)
                     throw new IllegalArgumentException("note outside the grid: x=" + x + " key=" + key);
                  cursorClip.setStep(x, key, n.integer("velocity"), n.num("duration"));
               }
               later(300, r, () -> r.ok(clipSnapshot()));
            });
         });
      });
   }

   private void readNotes(final Json.Obj q, final Reply r)
   {
      final double step = q.num("step");
      focusClip(q, r, () -> {
         final boolean changed = step != gridStep;
         setGrid(step);
         later(changed ? 400 : 50, r, () -> r.ok(clipSnapshot()));
      });
   }

   private void setClip(final Json.Obj q, final Reply r)
   {
      focusClip(q, r, () -> {
         if (q.has("name"))
            cursorClip.setName(q.str("name"));
         if (q.has("color"))
            setColor(cursorClip.color(), q.list("color"));
         if (q.has("loop_length"))
            cursorClip.getLoopLength().set(q.num("loop_length"));
         if (q.has("loop_enabled"))
            cursorClip.isLoopEnabled().set(q.bool("loop_enabled"));
         if (q.has("transpose"))
            cursorClip.transpose(q.integer("transpose"));
         if (q.has("quantize"))
            cursorClip.quantize(clamp01(q.num("quantize")));
         later(SETTLE_MS, r, () -> {
            final Map<String, Object> m = clipSnapshot();
            m.remove("notes");
            r.ok(m);
         });
      });
   }

   // ---------------------------------------------------------------- devices

   private void insertDevice(final Json.Obj q, final Reply r)
   {
      final Addr a = addr(q.obj("track"));
      final String at = q.str("at");
      final InsertionPoint ip;
      switch (at)
      {
         case "end":
            ip = a.track.endOfDeviceChainInsertionPoint();
            break;
         case "start":
            ip = a.track.startOfDeviceChainInsertionPoint();
            break;
         case "after":
            ip = device(a, q.integer("device")).afterDeviceInsertionPoint();
            break;
         case "before":
            ip = device(a, q.integer("device")).beforeDeviceInsertionPoint();
            break;
         case "replace":
            ip = device(a, q.integer("device")).replaceDeviceInsertionPoint();
            break;
         default:
            throw new IllegalArgumentException("unknown placement '" + at + "'");
      }
      final Json.Obj src = q.obj("source");
      final String kind = src.str("kind");
      final Runnable done = () -> later(500, r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("devices", devicesSnapshot(a));
         r.ok(m);
      });
      switch (kind)
      {
         case "clap":
            ip.insertCLAPDevice(src.str("id"));
            done.run();
            return;
         case "vst3":
            ip.insertVST3Device(src.str("id"));
            done.run();
            return;
         case "file":
            ip.insertFile(src.str("path"));
            done.run();
            return;
         case "browser":
            browse(ip, src.str("name"), src.strOr("content_type", "Devices"), r, done);
            return;
         default:
            throw new IllegalArgumentException("unknown device source '" + kind + "'");
      }
   }

   /** Opens the popup browser at {@code ip} and commits the result named {@code name}. */
   private void browse(final InsertionPoint ip, final String name, final String contentType,
      final Reply r, final Runnable done)
   {
      if (browser.exists().get())
         browser.cancel();
      ip.browse();
      waitFor(() -> browser.exists().get(), 3000, "the browser to open", r, () -> {
         final String[] types = browser.contentTypeNames().get();
         int t = -1;
         for (int i = 0; i < types.length; i++)
            if (types[i].equalsIgnoreCase(contentType))
               t = i;
         if (t < 0)
         {
            browser.cancel();
            throw new IllegalArgumentException("no browser tab '" + contentType + "'; have "
               + String.join(", ", types));
         }
         final int tab = t;
         browser.selectedContentTypeIndex().set(tab);
         resetFilters();
         results.scrollPosition().set(0);
         waitFor(() -> browser.selectedContentTypeIndex().get() == tab
            && results.itemCount().get() > 0 && results.getItemAt(0).exists().get(), 5000,
            "browser results", r,
            () -> later(300, r, () -> scan(name.toLowerCase(Locale.ROOT), 0, -1, new ArrayList<>(), r, done)));
      });
   }

   private static final int MAX_PAGES = 150;

   /** Fills one pad (scrolled to 0, so index = key); an occupied pad is cleared first. */
   private void insertPad(final Json.Obj q, final int key, final Reply r)
   {
      final com.bitwig.extension.controller.api.DrumPad pad = drumPads.getItemAt(key);
      final boolean occupied = pad.exists().get() && !pad.name().get().isEmpty();
      if (occupied)
         pad.deleteObject();
      later(occupied ? 300 : 0, r, () -> {
         final InsertionPoint ip = pad.insertionPoint();
         final Json.Obj src = q.obj("source");
         final Runnable done = () -> later(600, r, () -> r.ok(cursorDeviceSnapshot()));
         switch (src.str("kind"))
         {
            case "browser":
               browse(ip, src.str("name"), src.strOr("content_type", "Devices"), r, done);
               return;
            case "clap":
               ip.insertCLAPDevice(src.str("id"));
               done.run();
               return;
            case "vst3":
               ip.insertVST3Device(src.str("id"));
               done.run();
               return;
            case "file":
               ip.insertFile(src.str("path"));
               done.run();
               return;
            default:
               throw new IllegalArgumentException("unknown source");
         }
      });
   }

   /** Bitwig keeps browser filters between uses: clear them so earlier searches don't hide
    * results (seen: an audio-FX device type left from browsing on the master). */
   private void resetFilters()
   {
      for (final com.bitwig.extension.controller.api.BrowserFilterItem any : wildcards)
         any.isSelected().set(true);
   }

   /** Opens the browser on a track, collects result names page by page, then cancels. */
   private void listBrowser(final Addr a, final String contentType, final int max, final Reply r)
   {
      if (browser.exists().get())
         browser.cancel();
      a.track.endOfDeviceChainInsertionPoint().browse();
      waitFor(() -> browser.exists().get(), 3000, "the browser to open", r, () -> {
         final String[] types = browser.contentTypeNames().get();
         int t = -1;
         for (int i = 0; i < types.length; i++)
            if (types[i].equalsIgnoreCase(contentType))
               t = i;
         if (t < 0)
         {
            browser.cancel();
            throw new IllegalArgumentException("no browser tab '" + contentType + "'; have "
               + String.join(", ", types));
         }
         final int tab = t;
         browser.selectedContentTypeIndex().set(tab);
         resetFilters();
         results.scrollPosition().set(0);
         waitFor(() -> browser.selectedContentTypeIndex().get() == tab
            && results.itemCount().get() > 0 && results.getItemAt(0).exists().get(), 5000,
            "browser results", r, () -> later(300, r, () -> collect(new ArrayList<>(), max, 0, r)));
      });
   }

   /** Scrolls the results one page and continues once Bitwig has moved and refreshed the
    * names: reading on a timer skipped or repeated pages (seen: presets missing from
    * list_devices). */
   private void nextPage(final Reply r, final Runnable then)
   {
      final int before = results.scrollPosition().get();
      results.scrollPageForwards();
      waitFor(() -> results.scrollPosition().get() != before, 3000, "the next results page", r,
         () -> later(250, r, then));
   }

   private void collect(final List<String> names, final int max, final int page, final Reply r)
   {
      for (int i = 0; i < RESULTS && names.size() < max; i++)
      {
         final BrowserResultsItem it = results.getItemAt(i);
         if (it.exists().get())
            names.add(it.name().get());
      }
      if (names.size() < max && results.canScrollForwards().get() && page < MAX_PAGES)
      {
         nextPage(r, () -> collect(names, max, page + 1, r));
         return;
      }
      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("total", results.itemCount().get());
      m.put("names", new ArrayList<>(new java.util.LinkedHashSet<>(names)));
      browser.cancel();
      r.ok(m);
   }

   /** Pages through the results: an exact name wins; else the first prefix match. */
   private void scan(final String needle, final int page, final int prefixAt,
      final List<String> similar, final Reply r, final Runnable done)
   {
      int prefix = prefixAt;
      final int base = results.scrollPosition().get();
      for (int i = 0; i < RESULTS; i++)
      {
         final BrowserResultsItem it = results.getItemAt(i);
         if (!it.exists().get())
            continue;
         final String n = it.name().get().toLowerCase(Locale.ROOT);
         if (n.equals(needle))
         {
            commit(it, r, done);
            return;
         }
         if (prefix < 0 && n.startsWith(needle))
            prefix = base + i;
         if (similar.size() < 10 && n.contains(needle))
            similar.add(it.name().get());
      }
      final int found = prefix;
      if (results.canScrollForwards().get() && page < MAX_PAGES)
      {
         nextPage(r, () -> scan(needle, page + 1, found, similar, r, done));
         return;
      }
      if (found < 0)
      {
         browser.cancel();
         final List<String> seen = new ArrayList<>();
         for (int i = 0; i < Math.min(5, RESULTS); i++)
            if (results.getItemAt(i).exists().get())
               seen.add(results.getItemAt(i).name().get());
         r.fail("no browser result named '" + needle + "' among " + results.itemCount().get()
            + " results" + (similar.isEmpty() ? "" : "; similar: " + String.join(", ", similar))
            + (seen.isEmpty() ? "" : "; e.g. " + String.join(", ", seen)));
         return;
      }
      results.scrollPosition().set(found);
      later(250, r, () -> commit(results.getItemAt(0), r, done));
   }

   private void commit(final BrowserResultsItem it, final Reply r, final Runnable done)
   {
      it.isSelected().set(true);
      later(150, r, () -> {
         browser.commit();
         done.run();
      });
   }

   /** Points the cursor device at (track, device) and the remote-control page, then runs then. */
   private void focusDevice(final Json.Obj q, final Reply r, final Runnable then)
   {
      final Addr a = addr(q.obj("track"));
      final int d = q.integer("device");
      final Device dev = device(a, d);
      cursorTrack.selectChannel(a.track);
      cursorDevice.selectDevice(dev);
      waitFor(() -> a.selected.get() && cursorDevice.exists().get()
         && cursorDevice.position().get() == d, 3000, "the device to focus", r, () -> {
            if (q.has("page"))
            {
               final int p = q.integer("page");
               if (p < 0 || p >= remotePage.pageCount().get())
                  throw new IllegalArgumentException("page " + (p + 1) + " does not exist; have "
                     + String.join(", ", remotePage.pageNames().get()));
               remotePage.selectedPageIndex().set(p);
               waitFor(() -> remotePage.selectedPageIndex().get() == p, 2000, "the page", r,
                  () -> later(100, r, then));
            }
            else
               later(50, r, then);
         });
   }

   @SuppressWarnings("deprecation") // preset switching has no replacement in the API
   private void deviceAction(final Json.Obj q, final Reply r)
   {
      final Addr a = addr(q.obj("track"));
      final Device d = device(a, q.integer("device"));
      final String action = q.str("action");
      switch (action)
      {
         case "enable":
            d.isEnabled().set(true);
            break;
         case "disable":
            d.isEnabled().set(false);
            break;
         case "toggle":
            d.isEnabled().toggle();
            break;
         case "delete":
            d.deleteObject();
            break;
         case "duplicate":
            d.duplicateObject();
            break;
         case "open_window":
            d.isWindowOpen().set(true);
            break;
         case "close_window":
            d.isWindowOpen().set(false);
            break;
         case "next_preset":
            d.switchToNextPreset();
            break;
         case "previous_preset":
            d.switchToPreviousPreset();
            break;
         case "select":
            cursorTrack.selectChannel(a.track);
            cursorDevice.selectDevice(d);
            break;
         default:
            throw new IllegalArgumentException("unknown device action '" + action + "'");
      }
      later(300, r, () -> {
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("devices", devicesSnapshot(a));
         r.ok(m);
      });
   }

   // ---------------------------------------------------------------- MIDI, app

   private void playNotes(final Json.Obj q, final Reply r)
   {
      final Track t = addr(q.obj("track")).track;
      final int velocity = q.integer("velocity");
      final List<Integer> keys = new ArrayList<>();
      for (final Object o : q.list("keys"))
         keys.add(intOf(o));
      for (final int k : keys)
         t.startNote(k, velocity);
      final int ms = Math.max(10, Math.min(10000, q.integer("duration_ms")));
      later(ms, r, () -> {
         for (final int k : keys)
            t.stopNote(k, 0);
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("played", keys);
         m.put("duration_ms", ms);
         r.ok(m);
      });
   }

   private List<Object> listActions(final String query)
   {
      final String[] words = query.toLowerCase(Locale.ROOT).trim().split("\\s+");
      final List<Object> out = new ArrayList<>();
      for (final Action a : app.getActions())
      {
         final String hay = (a.getId() + " " + a.getName() + " " + a.getCategory().getName())
            .toLowerCase(Locale.ROOT);
         boolean all = true;
         for (final String w : words)
            if (!w.isEmpty() && !hay.contains(w))
               all = false;
         if (!all)
            continue;
         final Map<String, Object> m = new LinkedHashMap<>();
         m.put("id", a.getId());
         m.put("name", a.getName());
         m.put("category", a.getCategory().getName());
         out.add(m);
         if (out.size() >= 100)
            break;
      }
      return out;
   }

   private void setView(final Json.Obj q)
   {
      if (q.has("layout"))
         app.setPanelLayout(q.str("layout"));
      if (!q.has("toggle"))
         return;
      final String p = q.str("toggle");
      switch (p)
      {
         case "inspector":
            app.toggleInspector();
            break;
         case "devices":
            app.toggleDevices();
            break;
         case "mixer":
            app.toggleMixer();
            break;
         case "note_editor":
            app.toggleNoteEditor();
            break;
         case "automation_editor":
            app.toggleAutomationEditor();
            break;
         case "browser":
            app.toggleBrowserVisibility();
            break;
         case "fullscreen":
            app.toggleFullScreen();
            break;
         default:
            throw new IllegalArgumentException("unknown panel '" + p + "'");
      }
   }
}
