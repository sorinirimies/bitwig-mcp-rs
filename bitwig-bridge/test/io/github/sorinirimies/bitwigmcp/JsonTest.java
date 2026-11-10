package io.github.sorinirimies.bitwigmcp;

import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Plain-Java tests for Json (no Bitwig, no JUnit): `nu scripts/bridge.nu test`. */
public final class JsonTest
{
   private static int failures;

   private static void check(final boolean ok, final String what)
   {
      if (!ok)
      {
         failures++;
         System.err.println("FAIL: " + what);
      }
   }

   private static void throwsOn(final String text)
   {
      try
      {
         Json.parse(text);
         check(false, "should reject: " + text);
      }
      catch (final IllegalArgumentException e)
      {
         // expected
      }
   }

   public static void main(final String[] args)
   {
      final Json.Obj req = new Json.Obj(Json.parse(
         "{\"id\": 7, \"cmd\": \"set_track\", \"track\": {\"bank\": \"main\", \"index\": 0},"
            + " \"volume\": 0.5, \"mute\": true, \"name\": \"B\\u00e4ss \\\"x\\\"\\n\","
            + " \"sends\": [{\"index\": 1, \"volume\": 1e-1}], \"nothing\": null}"));
      check(req.integer("id") == 7, "id");
      check(req.str("cmd").equals("set_track"), "cmd");
      check(req.obj("track").str("bank").equals("main"), "nested");
      check(req.num("volume") == 0.5, "number");
      check(req.bool("mute"), "bool");
      check(req.str("name").equals("Bäss \"x\"\n"), "escapes");
      check(new Json.Obj(req.list("sends").get(0)).num("volume") == 0.1, "exponent");
      check(!req.has("nothing") && !req.has("absent"), "null and absent");
      check(req.list("absent").isEmpty(), "absent list");
      try
      {
         req.str("volume");
         check(false, "type check");
      }
      catch (final IllegalArgumentException e)
      {
         check(e.getMessage().contains("volume"), "error names the field");
      }

      final Map<String, Object> m = new LinkedHashMap<>();
      m.put("id", 7);
      m.put("ok", true);
      m.put("f", 0.25);
      m.put("whole", 3.0);
      m.put("nan", Double.NaN);
      m.put("s", "a\"b\\c\n\u0001");
      m.put("list", Arrays.asList(1, "x", null));
      m.put("arr", new String[] { "p", "q" });
      final String out = Json.write(m);
      check(out.equals("{\"id\":7,\"ok\":true,\"f\":0.25,\"whole\":3,\"nan\":null,"
         + "\"s\":\"a\\\"b\\\\c\\n\\u0001\",\"list\":[1,\"x\",null],\"arr\":[\"p\",\"q\"]}"),
         "write: " + out);
      final Json.Obj back = new Json.Obj(Json.parse(out));
      check(back.str("s").equals("a\"b\\c\n\u0001"), "round trip");
      check(((List<?>) back.raw("arr")).size() == 2, "array round trip");

      for (final String bad : new String[] { "", "{", "{\"a\"}", "[1,]", "tru", "\"x", "{} x",
         "{\"a\":1,}", "-", "\"\\q\"" })
         throwsOn(bad);

      if (failures > 0)
      {
         System.err.println(failures + " failure(s)");
         System.exit(1);
      }
      System.out.println("JsonTest: all passed");
   }
}
