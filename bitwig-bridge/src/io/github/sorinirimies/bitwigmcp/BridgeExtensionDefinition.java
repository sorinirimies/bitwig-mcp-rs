package io.github.sorinirimies.bitwigmcp;

import java.util.UUID;

import com.bitwig.extension.api.PlatformType;
import com.bitwig.extension.controller.AutoDetectionMidiPortNamesList;
import com.bitwig.extension.controller.ControllerExtensionDefinition;
import com.bitwig.extension.controller.api.ControllerHost;

/** Registers the bridge with Bitwig (Settings → Controllers → bitwig-mcp-rs → MCP bridge). */
public class BridgeExtensionDefinition extends ControllerExtensionDefinition
{
   /** The workspace version (Cargo.toml), written into BuildInfo by scripts/bridge.nu. */
   public static final String VERSION = BuildInfo.VERSION;

   /** Bridge protocol version, from protocol.schema.json via BuildInfo. */
   public static final int PROTOCOL = BuildInfo.PROTOCOL;

   private static final UUID ID = UUID.fromString("90f16768-24e2-479e-9264-46a004e3088b");

   @Override
   public String getName()
   {
      return "MCP bridge";
   }

   @Override
   public String getAuthor()
   {
      return "sorinirimies";
   }

   @Override
   public String getVersion()
   {
      return VERSION;
   }

   @Override
   public UUID getId()
   {
      return ID;
   }

   @Override
   public String getHardwareVendor()
   {
      return "bitwig-mcp-rs";
   }

   @Override
   public String getHardwareModel()
   {
      return "MCP bridge";
   }

   @Override
   public int getRequiredAPIVersion()
   {
      return 18;
   }

   @Override
   public int getNumMidiInPorts()
   {
      return 0;
   }

   /**
    * No MIDI ports: Bitwig would otherwise grab a port that another controller may own and
    * refuse to start the bridge. SysEx gets its own product, {@link SysexBridgeExtensionDefinition}.
    */
   @Override
   public int getNumMidiOutPorts()
   {
      return 0;
   }

   @Override
   public void listAutoDetectionMidiPortNames(final AutoDetectionMidiPortNamesList list,
      final PlatformType platformType)
   {
      // No hardware: added by hand once.
   }

   @Override
   public BridgeExtension createInstance(final ControllerHost host)
   {
      return new BridgeExtension(this, host);
   }
}
