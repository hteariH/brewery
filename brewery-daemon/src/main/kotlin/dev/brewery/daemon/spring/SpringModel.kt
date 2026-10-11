package dev.brewery.daemon.spring

import com.google.gson.TypeAdapter
import com.google.gson.annotations.JsonAdapter
import com.google.gson.stream.JsonReader
import com.google.gson.stream.JsonToken
import com.google.gson.stream.JsonWriter

enum class SpringBeanKind { component, service, repository, controller, restController, configuration, bean }

enum class HttpMethod { GET, POST, PUT, DELETE, PATCH, ANY }

// lsp4j's Gson writes enums as ordinals; the wire format wants the name (same reason as BuildToolAdapter).
abstract class NameEnumAdapter<E : Enum<E>>(private val type: Class<E>) : TypeAdapter<E>() {
    override fun write(out: JsonWriter, value: E?) {
        if (value == null) out.nullValue() else out.value(value.name)
    }

    override fun read(reader: JsonReader): E? =
        if (reader.peek() == JsonToken.NULL) {
            reader.nextNull()
            null
        } else {
            java.lang.Enum.valueOf(type, reader.nextString())
        }
}

class SpringBeanKindAdapter : NameEnumAdapter<SpringBeanKind>(SpringBeanKind::class.java)
class HttpMethodAdapter : NameEnumAdapter<HttpMethod>(HttpMethod::class.java)

/** [file] is absolute, [line] 1-based and points at the annotated declaration. */
data class SpringApp(val name: String, val mainClass: String, val modulePath: String, val file: String, val line: Int)

data class SpringBean(
    val name: String,
    val type: String,
    @field:JsonAdapter(SpringBeanKindAdapter::class) val kind: SpringBeanKind,
    val file: String,
    val line: Int,
)

/** [handler] is `SimpleClass#method`. */
data class SpringEndpoint(
    @field:JsonAdapter(HttpMethodAdapter::class) val method: HttpMethod,
    val path: String,
    val handler: String,
    val file: String,
    val line: Int,
)

data class SpringModel(
    val applications: List<SpringApp>,
    val beans: List<SpringBean>,
    val endpoints: List<SpringEndpoint>,
)
