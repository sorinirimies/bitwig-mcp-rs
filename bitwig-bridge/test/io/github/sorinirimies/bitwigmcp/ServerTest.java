package io.github.sorinirimies.bitwigmcp;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

/** Round trip through Server: a client frame is queued; a reply frame reaches the client. */
public final class ServerTest
{
   public static void main(final String[] args) throws Exception
   {
      try (Server server = new Server(0, msg -> System.err.println("log: " + msg));
           Socket client = new Socket("127.0.0.1", server.port()))
      {
         final DataOutputStream out = new DataOutputStream(client.getOutputStream());
         final byte[] hello = "{\"id\":1,\"cmd\":\"hello\"}".getBytes(StandardCharsets.UTF_8);
         out.writeInt(hello.length);
         out.write(hello);
         out.flush();

         Server.Incoming in = null;
         for (int i = 0; i < 200 && in == null; i++)
         {
            in = server.poll();
            if (in == null)
               Thread.sleep(10);
         }
         if (in == null || !new String(in.data, StandardCharsets.UTF_8).contains("hello"))
            throw new AssertionError("frame not queued");

         in.conn.send("{\"id\":1,\"ok\":true}".getBytes(StandardCharsets.UTF_8));
         final DataInputStream din = new DataInputStream(client.getInputStream());
         final byte[] reply = new byte[din.readInt()];
         din.readFully(reply);
         if (!new String(reply, StandardCharsets.UTF_8).equals("{\"id\":1,\"ok\":true}"))
            throw new AssertionError("bad reply");

         // oversized frames drop the client instead of allocating
         out.writeInt(Server.MAX_FRAME + 1);
         out.flush();
         client.setSoTimeout(2000);
         if (din.read() != -1)
            throw new AssertionError("oversized frame did not close the connection");
      }
      System.out.println("ServerTest: all passed");
   }
}
