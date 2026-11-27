#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlerror.h>

#include "base/json/json_reader.h"
#include "base/json/json_writer.h"
#include "base/values.h"

namespace {

using Bytes = std::vector<uint8_t>;

void number(Bytes& out, uint32_t value) {
  for (int shift = 24; shift >= 0; shift -= 8) out.push_back(value >> shift);
}

void number32(Bytes& out, int32_t value) {
  number(out, static_cast<uint32_t>(value));
}

void number64(Bytes& out, uint64_t value) {
  for (int shift = 56; shift >= 0; shift -= 8) out.push_back(value >> shift);
}

void blob(Bytes& out, std::string_view value) {
  number(out, static_cast<uint32_t>(value.size()));
  out.insert(out.end(), value.begin(), value.end());
}

void dump_value(const base::Value& value, Bytes& out) {
  if (value.is_none()) {
    out.push_back(0);
  } else if (value.is_bool()) {
    out.push_back(value.GetBool() ? 2 : 1);
  } else if (value.is_int()) {
    out.push_back(3);
    number32(out, value.GetInt());
  } else if (value.is_double()) {
    out.push_back(4);
    uint64_t bits = 0;
    double number = value.GetDouble();
    std::memcpy(&bits, &number, sizeof(bits));
    number64(out, bits);
  } else if (value.is_string()) {
    out.push_back(5);
    blob(out, value.GetString());
  } else if (value.is_list()) {
    out.push_back(6);
    const auto& list = value.GetList();
    number(out, static_cast<uint32_t>(list.size()));
    for (const auto& item : list) dump_value(item, out);
  } else if (value.is_dict()) {
    out.push_back(7);
    const auto& dict = value.GetDict();
    number(out, static_cast<uint32_t>(dict.size()));
    for (const auto& [key, item] : dict) {
      blob(out, key);
      dump_value(item, out);
    }
  } else {
    std::abort();
  }
}

struct Reader {
  const uint8_t* data;
  size_t size;
  size_t offset = 0;

  uint8_t byte() {
    if (offset >= size) std::abort();
    return data[offset++];
  }
  uint32_t word() {
    if (size - offset < 4) std::abort();
    uint32_t result = 0;
    for (int i = 0; i < 4; ++i) result = result * 256 + byte();
    return result;
  }
  std::string_view bytes() {
    uint32_t length = word();
    if (length > size - offset) std::abort();
    auto result = std::string_view(reinterpret_cast<const char*>(data + offset), length);
    offset += length;
    return result;
  }
};

base::Value read_value(Reader& reader, size_t depth) {
  if (depth > 200) std::abort();
  uint8_t tag = reader.byte();
  switch (tag) {
    case 0:
      return base::Value();
    case 1:
      return base::Value(false);
    case 2:
      return base::Value(true);
    case 3: {
      uint32_t bits = reader.word();
      int32_t value = 0;
      std::memcpy(&value, &bits, sizeof(value));
      return base::Value(static_cast<int>(value));
    }
    case 4: {
      uint64_t bits = 0;
      for (int i = 0; i < 8; ++i) bits = bits * 256 + reader.byte();
      double value = 0;
      std::memcpy(&value, &bits, sizeof(value));
      return base::Value(value);
    }
    case 5: {
      auto text = reader.bytes();
      return base::Value(std::string(text));
    }
    case 6: {
      uint32_t count = reader.word();
      if (count > 100000) std::abort();
      base::ListValue list;
      for (uint32_t i = 0; i < count; ++i) list.Append(read_value(reader, depth + 1));
      return base::Value(std::move(list));
    }
    case 7: {
      uint32_t count = reader.word();
      if (count > 100000) std::abort();
      base::DictValue dict;
      for (uint32_t i = 0; i < count; ++i) {
        auto key = reader.bytes();
        dict.Set(std::string(key), read_value(reader, depth + 1));
      }
      return base::Value(std::move(dict));
    }
    default:
      std::abort();
  }
}

Bytes json_parse(const uint8_t* data, size_t size) {
  if (size < 5) std::abort();
  uint8_t options = data[0];
  if (options > 63) std::abort();
  uint32_t length = 0;
  for (int i = 0; i < 4; ++i) length = length * 256 + data[1 + i];
  if (length != size - 5) std::abort();
  std::string_view input(reinterpret_cast<const char*>(data + 5), length);
  auto result = base::JSONReader::ReadAndReturnValueWithError(input, options);
  Bytes out;
  if (result.has_value()) {
    out.push_back(1);
    dump_value(result.value(), out);
    return out;
  }
  out.push_back(0);
  number32(out, result.error().line);
  number32(out, result.error().column);
  blob(out, result.error().message);
  return out;
}

Bytes json_write(const uint8_t* data, size_t size) {
  if (size < 2) std::abort();
  uint8_t options = data[0];
  if (options > 3) std::abort();
  Reader reader{data + 1, size - 1};
  base::Value value = read_value(reader, 0);
  if (reader.offset != reader.size) std::abort();
  int writer_options = 0;
  if (options & 1) writer_options |= base::JSONWriter::OPTIONS_OMIT_DOUBLE_TYPE_PRESERVATION;
  if (options & 2) writer_options |= base::JSONWriter::OPTIONS_PRETTY_PRINT;
  std::string text;
  bool ok = base::JSONWriter::WriteWithOptions(value, writer_options, &text, 200);
  Bytes out;
  out.push_back(ok ? 1 : 0);
  blob(out, text);
  return out;
}

std::string xml_kind(xmlNodePtr node) {
  const xmlChar* name = nullptr;
  if (node->type == XML_ELEMENT_NODE) {
    name = node->name;
    if (node->ns != nullptr && node->ns->prefix != nullptr) {
      return std::string(reinterpret_cast<const char*>(node->ns->prefix)) + ":" +
             reinterpret_cast<const char*>(name);
    }
  } else if (node->type == XML_PI_NODE) {
    name = node->name;
  }
  return name == nullptr ? std::string() : std::string(reinterpret_cast<const char*>(name));
}

void emit_node(xmlNodePtr node, Bytes& out, uint32_t& events) {
  while (node != nullptr) {
    if (node->type == XML_ELEMENT_NODE) {
      ++events;
      out.push_back(1);
      blob(out, xml_kind(node));
      uint32_t attributes = 0;
      for (xmlAttrPtr attribute = node->properties; attribute != nullptr; attribute = attribute->next) ++attributes;
      number(out, attributes);
      for (xmlAttrPtr attribute = node->properties; attribute != nullptr; attribute = attribute->next) {
        std::string name;
        if (attribute->ns != nullptr && attribute->ns->prefix != nullptr) {
          name = std::string(reinterpret_cast<const char*>(attribute->ns->prefix)) + ":";
        }
        name += reinterpret_cast<const char*>(attribute->name);
        blob(out, name);
        std::string text;
        if (attribute->children != nullptr && attribute->children->content != nullptr) {
          text = reinterpret_cast<const char*>(attribute->children->content);
        }
        blob(out, text);
      }
      uint32_t namespaces = 0;
      for (xmlNsPtr space = node->nsDef; space != nullptr; space = space->next) ++namespaces;
      number(out, namespaces);
      for (xmlNsPtr space = node->nsDef; space != nullptr; space = space->next) {
        blob(out, space->prefix == nullptr ? "" : reinterpret_cast<const char*>(space->prefix));
        blob(out, space->href == nullptr ? "" : reinterpret_cast<const char*>(space->href));
      }
      emit_node(node->children, out, events);
      ++events;
      out.push_back(2);
      blob(out, xml_kind(node));
    } else if (node->type == XML_TEXT_NODE) {
      ++events;
      out.push_back(3);
      blob(out, node->content == nullptr ? "" : reinterpret_cast<const char*>(node->content));
    } else if (node->type == XML_COMMENT_NODE) {
      ++events;
      out.push_back(4);
      blob(out, node->content == nullptr ? "" : reinterpret_cast<const char*>(node->content));
    } else if (node->type == XML_PI_NODE) {
      ++events;
      out.push_back(5);
      blob(out, xml_kind(node));
      blob(out, node->content == nullptr ? "" : reinterpret_cast<const char*>(node->content));
    } else if (node->type == XML_CDATA_SECTION_NODE) {
      ++events;
      out.push_back(6);
      blob(out, node->content == nullptr ? "" : reinterpret_cast<const char*>(node->content));
    }
    node = node->next;
  }
}

std::string xml_error_text;

void xml_error(void*, const char* message, ...) {
  va_list args;
  va_start(args, message);
  char buffer[2048];
  vsnprintf(buffer, sizeof(buffer), message, args);
  va_end(args);
  xml_error_text += buffer;
  xml_error_text += '\n';
}

Bytes xml_parse(const uint8_t* data, size_t size) {
  if (size < 8) std::abort();
  uint32_t options = 0;
  for (int i = 0; i < 4; ++i) options = options * 256 + data[i];
  uint32_t length = 0;
  for (int i = 0; i < 4; ++i) length = length * 256 + data[4 + i];
  if (length != size - 8 || length > 16 * 1024 * 1024) std::abort();
  xml_error_text.clear();
  xmlSetGenericErrorFunc(nullptr, xml_error);
  xmlDocPtr document = xmlReadMemory(reinterpret_cast<const char*>(data + 8), static_cast<int>(length),
                                    nullptr, nullptr, static_cast<int>(options));
  xmlSetGenericErrorFunc(nullptr, nullptr);
  Bytes out;
  if (document == nullptr) {
    out.push_back(0);
    return out;
  }
  Bytes events;
  uint32_t count = 0;
  emit_node(xmlDocGetRootElement(document), events, count);
  xmlChar* serialized = nullptr;
  int serialized_length = 0;
  xmlDocDumpMemory(document, &serialized, &serialized_length);
  out.push_back(1);
  number(out, count);
  out.insert(out.end(), events.begin(), events.end());
  blob(out, std::string_view(reinterpret_cast<const char*>(serialized), serialized_length));
  if (serialized != nullptr) xmlFree(serialized);
  xmlFreeDoc(document);
  return out;
}

}  // namespace

int main() {
  char header[4];
  if (std::fread(header, 1, 4, stdin) != 4 || std::string(header, 4) != "MGB1") return 1;
  std::fwrite(header, 1, 4, stdout);
  std::fflush(stdout);
  while (std::fread(header, 1, 4, stdin) == 4) {
    uint32_t size = 0;
    for (char byte : header) size = size * 256 + static_cast<uint8_t>(byte);
    if (size > 16 * 1024 * 1024) return 1;
    Bytes input(size);
    if (std::fread(input.data(), 1, size, stdin) != size) return 1;
    Bytes output;
    if (input.empty()) std::abort();
    if (input[0] == 1) output = json_parse(input.data() + 1, input.size() - 1);
    else if (input[0] == 2) output = json_write(input.data() + 1, input.size() - 1);
    else if (input[0] == 3) output = xml_parse(input.data() + 1, input.size() - 1);
    else std::abort();
    Bytes length;
    number(length, static_cast<uint32_t>(output.size()));
    std::fwrite(length.data(), 1, 4, stdout);
    std::fwrite(output.data(), 1, output.size(), stdout);
    std::fflush(stdout);
  }
}
