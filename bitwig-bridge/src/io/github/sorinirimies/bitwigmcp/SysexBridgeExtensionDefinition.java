package io.github.sorinirimies.bitwigmcp;

import java.util.UUID;

/**
 * The bridge plus one MIDI output for SysEx (hardware synth settings; Track MIDI cannot carry
 * SysEx). Add this product instead of "MCP bridge" and give it your synth's MIDI port.
 */
public class SysexBridgeExtensionDefinition extends BridgeExtensionDefinition
{
   private static final UUID ID = UUID.fromString("5a3c1f0e-7b2d-4e8a-9c61-2f4d8b9e0a17");

   @Override
   public String getName()
   {
      return "MCP bridge + SysEx out";
   }

   @Override
   public UUID getId()
   {
      return ID;
   }

   @Override
   public String getHardwareModel()
   {
      return "MCP bridge + SysEx out";
   }

   @Override
   public int getNumMidiOutPorts()
   {
      return 1;
   }
}
