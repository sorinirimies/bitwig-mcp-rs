package io.github.sorinirimies.bitwigmcp;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.util.List;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.function.Consumer;

/**
 * The bridge's TCP server: plain Java sockets on 127.0.0.1, one reader thread per client.
 *
 * Frames are a 32-bit big-endian length plus that many bytes of UTF-8 JSON, both ways.
 * Readers only queue frames; the extension drains the queue on Bitwig's control thread
 * ({@link #poll()}), so every Controller API call stays on that thread. (Bitwig's own
 * RemoteSocket accepted connections but never delivered their data in Bitwig 6.)
 */
final class Server implements AutoCloseable
{
   /** Largest frame accepted: 16 MiB. */
   static final int MAX_FRAME = 16 * 1024 * 1024;

   /** One connected client. */
   static final class Conn
   {
      private final Socket socket;
      private final DataOutputStream out;

      Conn(final Socket socket) throws IOException
      {
         this.socket = socket;
         this.out = new DataOutputStream(socket.getOutputStream());
      }

      /** Writes one frame. Safe from any thread. */
      synchronized void send(final byte[] payload) throws IOException
      {
         out.writeInt(payload.length);
         out.write(payload);
         out.flush();
      }

      void close()
      {
         try
         {
            socket.close();
         }
         catch (final IOException e)
         {
            // closing anyway
         }
      }
   }

   /** A frame from a client. */
   static final class Incoming
   {
      final Conn conn;
      final byte[] data;

      Incoming(final Conn conn, final byte[] data)
      {
         this.conn = conn;
         this.data = data;
      }
   }

   private final ServerSocket server;
   private final Consumer<String> log;
   private final ConcurrentLinkedQueue<Incoming> queue = new ConcurrentLinkedQueue<>();
   private final List<Conn> conns = new CopyOnWriteArrayList<>();
   private volatile boolean open = true;

   /** Binds 127.0.0.1:{@code port} and starts accepting. */
   Server(final int port, final Consumer<String> log) throws IOException
   {
      this.log = log;
      server = new ServerSocket();
      server.setReuseAddress(true);
      server.bind(new InetSocketAddress(InetAddress.getLoopbackAddress(), port));
      final Thread t = new Thread(this::acceptLoop, "bitwig-mcp-rs accept");
      t.setDaemon(true);
      t.start();
   }

   int port()
   {
      return server.getLocalPort();
   }

   /** The next frame, or null. */
   Incoming poll()
   {
      return queue.poll();
   }

   private void acceptLoop()
   {
      while (open)
      {
         try
         {
            final Socket s = server.accept();
            s.setTcpNoDelay(true);
            final Conn c = new Conn(s);
            conns.add(c);
            final Thread t = new Thread(() -> readLoop(c, s), "bitwig-mcp-rs client");
            t.setDaemon(true);
            t.start();
         }
         catch (final IOException e)
         {
            if (open)
               log.accept("accept failed: " + e.getMessage());
         }
      }
   }

   private void readLoop(final Conn c, final Socket s)
   {
      try (DataInputStream in = new DataInputStream(s.getInputStream()))
      {
         while (open)
         {
            final int len = in.readInt();
            if (len < 0 || len > MAX_FRAME)
            {
               log.accept("dropping a client: frame of " + len + " bytes");
               break;
            }
            final byte[] data = new byte[len];
            in.readFully(data);
            queue.add(new Incoming(c, data));
         }
      }
      catch (final IOException e)
      {
         // client left
      }
      finally
      {
         conns.remove(c);
         c.close();
      }
   }

   @Override
   public void close()
   {
      open = false;
      try
      {
         server.close();
      }
      catch (final IOException e)
      {
         // closing anyway
      }
      for (final Conn c : conns)
         c.close();
   }
}
