package io.github.sorinirimies.bitwigmcp;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal JSON for the bridge protocol, so the extension has no dependencies.
 * Objects are {@code Map<String, Object>}, arrays {@code List<Object>}, numbers {@code Double}
 * (or any {@code Number} when writing), plus {@code String}, {@code Boolean} and {@code null}.
 */
public final class Json
{
   private final String s;
   private int i;

   private Json(final String s)
   {
      this.s = s;
   }

   /** Parses a JSON document; throws IllegalArgumentException on bad input. */
   public static Object parse(final String text)
   {
      final Json p = new Json(text);
      p.ws();
      final Object v = p.value();
      p.ws();
      if (p.i != p.s.length())
         throw p.error("trailing characters");
      return v;
   }

   private IllegalArgumentException error(final String what)
   {
      return new IllegalArgumentException("JSON: " + what + " at " + i);
   }

   private void ws()
   {
      while (i < s.length() && Character.isWhitespace(s.charAt(i)))
         i++;
   }

   private Object value()
   {
      if (i >= s.length())
         throw error("unexpected end");
      final char c = s.charAt(i);
      switch (c)
      {
         case '{':
            return object();
         case '[':
            return array();
         case '"':
            return string();
         case 't':
            return literal("true", Boolean.TRUE);
         case 'f':
            return literal("false", Boolean.FALSE);
         case 'n':
            return literal("null", null);
         default:
            if (c == '-' || (c >= '0' && c <= '9'))
               return number();
            throw error("unexpected '" + c + "'");
      }
   }

   private Object literal(final String word, final Object v)
   {
      if (!s.startsWith(word, i))
         throw error("expected " + word);
      i += word.length();
      return v;
   }

   private Map<String, Object> object()
   {
      final Map<String, Object> m = new LinkedHashMap<>();
      i++;
      ws();
      if (i < s.length() && s.charAt(i) == '}')
      {
         i++;
         return m;
      }
      while (true)
      {
         ws();
         if (i >= s.length() || s.charAt(i) != '"')
            throw error("expected key");
         final String k = string();
         ws();
         if (i >= s.length() || s.charAt(i) != ':')
            throw error("expected ':'");
         i++;
         ws();
         m.put(k, value());
         ws();
         if (i >= s.length())
            throw error("unexpected end");
         final char c = s.charAt(i++);
         if (c == '}')
            return m;
         if (c != ',')
            throw error("expected ',' or '}'");
      }
   }

   private List<Object> array()
   {
      final List<Object> l = new ArrayList<>();
      i++;
      ws();
      if (i < s.length() && s.charAt(i) == ']')
      {
         i++;
         return l;
      }
      while (true)
      {
         ws();
         l.add(value());
         ws();
         if (i >= s.length())
            throw error("unexpected end");
         final char c = s.charAt(i++);
         if (c == ']')
            return l;
         if (c != ',')
            throw error("expected ',' or ']'");
      }
   }

   private String string()
   {
      final StringBuilder b = new StringBuilder();
      i++;
      while (true)
      {
         if (i >= s.length())
            throw error("unterminated string");
         final char c = s.charAt(i++);
         if (c == '"')
            return b.toString();
         if (c != '\\')
         {
            b.append(c);
            continue;
         }
         if (i >= s.length())
            throw error("bad escape");
         final char e = s.charAt(i++);
         switch (e)
         {
            case '"':
            case '\\':
            case '/':
               b.append(e);
               break;
            case 'b':
               b.append('\b');
               break;
            case 'f':
               b.append('\f');
               break;
            case 'n':
               b.append('\n');
               break;
            case 'r':
               b.append('\r');
               break;
            case 't':
               b.append('\t');
               break;
            case 'u':
               if (i + 4 > s.length())
                  throw error("bad unicode escape");
               b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
               i += 4;
               break;
            default:
               throw error("bad escape");
         }
      }
   }

   private Double number()
   {
      final int start = i;
      while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0)
         i++;
      try
      {
         return Double.valueOf(s.substring(start, i));
      }
      catch (final NumberFormatException e)
      {
         throw error("bad number");
      }
   }

   /** Serializes maps, lists, strings, numbers, booleans, null and string arrays. */
   public static String write(final Object v)
   {
      final StringBuilder b = new StringBuilder();
      write(b, v);
      return b.toString();
   }

   private static void write(final StringBuilder b, final Object v)
   {
      if (v == null)
         b.append("null");
      else if (v instanceof String)
         quote(b, (String) v);
      else if (v instanceof Boolean)
         b.append(v.toString());
      else if (v instanceof Double || v instanceof Float)
      {
         final double d = ((Number) v).doubleValue();
         if (Double.isNaN(d) || Double.isInfinite(d))
            b.append("null");
         else if (d == Math.rint(d) && Math.abs(d) < 1e15)
            b.append((long) d);
         else
            b.append(d);
      }
      else if (v instanceof Number)
         b.append(((Number) v).longValue());
      else if (v instanceof Map)
      {
         b.append('{');
         boolean first = true;
         for (final Map.Entry<?, ?> e : ((Map<?, ?>) v).entrySet())
         {
            if (!first)
               b.append(',');
            first = false;
            quote(b, String.valueOf(e.getKey()));
            b.append(':');
            write(b, e.getValue());
         }
         b.append('}');
      }
      else if (v instanceof List)
      {
         b.append('[');
         boolean first = true;
         for (final Object o : (List<?>) v)
         {
            if (!first)
               b.append(',');
            first = false;
            write(b, o);
         }
         b.append(']');
      }
      else if (v instanceof String[])
      {
         final List<Object> l = new ArrayList<>();
         for (final String x : (String[]) v)
            l.add(x);
         write(b, l);
      }
      else
         quote(b, v.toString());
   }

   private static void quote(final StringBuilder b, final String s)
   {
      b.append('"');
      for (int k = 0; k < s.length(); k++)
      {
         final char c = s.charAt(k);
         switch (c)
         {
            case '"':
               b.append("\\\"");
               break;
            case '\\':
               b.append("\\\\");
               break;
            case '\n':
               b.append("\\n");
               break;
            case '\r':
               b.append("\\r");
               break;
            case '\t':
               b.append("\\t");
               break;
            default:
               if (c < 0x20)
                  b.append(String.format("\\u%04x", (int) c));
               else
                  b.append(c);
         }
      }
      b.append('"');
   }

   // ------------------------------------------------------------- typed access

   /** A field of a request, with type checks and readable errors. */
   public static final class Obj
   {
      private final Map<String, Object> m;

      public Obj(final Object o)
      {
         if (!(o instanceof Map))
            throw new IllegalArgumentException("expected an object");
         @SuppressWarnings("unchecked")
         final Map<String, Object> map = (Map<String, Object>) o;
         this.m = map;
      }

      public boolean has(final String k)
      {
         return m.containsKey(k) && m.get(k) != null;
      }

      public Object raw(final String k)
      {
         return m.get(k);
      }

      public String str(final String k)
      {
         final Object v = m.get(k);
         if (!(v instanceof String))
            throw new IllegalArgumentException("field '" + k + "' must be a string");
         return (String) v;
      }

      public String strOr(final String k, final String def)
      {
         return has(k) ? str(k) : def;
      }

      public double num(final String k)
      {
         final Object v = m.get(k);
         if (!(v instanceof Number))
            throw new IllegalArgumentException("field '" + k + "' must be a number");
         return ((Number) v).doubleValue();
      }

      public int integer(final String k)
      {
         return (int) Math.round(num(k));
      }

      public boolean bool(final String k)
      {
         final Object v = m.get(k);
         if (!(v instanceof Boolean))
            throw new IllegalArgumentException("field '" + k + "' must be a boolean");
         return (Boolean) v;
      }

      public Obj obj(final String k)
      {
         if (!has(k))
            throw new IllegalArgumentException("missing field '" + k + "'");
         return new Obj(m.get(k));
      }

      public List<Object> list(final String k)
      {
         final Object v = m.get(k);
         if (v == null)
            return new ArrayList<>();
         if (!(v instanceof List))
            throw new IllegalArgumentException("field '" + k + "' must be an array");
         @SuppressWarnings("unchecked")
         final List<Object> l = (List<Object>) v;
         return l;
      }
   }
}
