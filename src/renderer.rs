use crate::parser::{Node, Visibility};
use crate::scanner::Range;

type NodeIter<'a> = std::iter::Peekable<std::slice::Iter<'a, Node>>;

#[derive(Debug)]
pub enum RenderError {
    DuplicateParamName(String, Range),
}

#[derive(Debug)]
struct Context {
    pub builder_lines: String,
    pub imports: Vec<String>,
    pub functions: Vec<String>,
    pub typed_params: Vec<(String, String)>,
    pub includes_for_loop: bool,
    pub has_template_content: bool,
}

pub fn render(
    iter: &mut NodeIter,
    prog_name: &str,
    from_file_name: &str,
) -> Result<String, RenderError> {
    let context = render_lines(iter)?;

    let import_lines = context
        .imports
        .iter()
        .map(|details| format!("import {}", details))
        .collect::<Vec<_>>()
        .join("\n");

    let params_string = context
        .typed_params
        .iter()
        .map(|(param_name, type_name)| format!("{} {}: {}", param_name, param_name, type_name))
        .collect::<Vec<_>>()
        .join(", ");

    let args_string = context
        .typed_params
        .iter()
        .map(|(param_name, _)| format!("{}: {}", param_name, param_name))
        .collect::<Vec<_>>()
        .join(", ");

    let functions = if context.functions.is_empty() {
        String::new()
    } else {
        context.functions.join("\n\n")
    };

    let list_import = if context.includes_for_loop {
        "import gleam/list\n"
    } else {
        ""
    };

    let render_functions = if context.has_template_content {
        format!(
            r#"
pub fn render_builder({params_string}) -> StringBuilder {{
    let builder = string_builder.from_string("")
{builder_lines}
    builder
}}

pub fn render({params_string}) -> String {{
    string_builder.to_string(render_builder({args_string}))
}}
"#,
            params_string = params_string,
            builder_lines = context.builder_lines,
            args_string = args_string
        )
    } else {
        String::new()
    };

    let output = format!(
        r#"// DO NOT EDIT: Code generated by {prog_name} from {source_file}

import gleam/string_builder.{{type StringBuilder}}
{list_import}
{import_lines}{functions}
{render_functions}
"#,
        prog_name = prog_name,
        source_file = from_file_name,
        list_import = list_import,
        import_lines = import_lines,
        render_functions = render_functions,
    );

    Ok(output)
}

fn render_lines(iter: &mut NodeIter) -> Result<Context, RenderError> {
    let mut builder_lines = String::new();
    let mut imports = vec![];
    let mut functions = vec![];

    // Use a Vec<(String, String)> instead of a HashMap to maintain order which gives the users
    // some control, though parameters are labelled and can be called in any order. Some kind of
    // order is required to keep the tests passing as it seems to be non-determinate in a HashMap
    let mut typed_params = Vec::new();
    let mut includes_for_loop = false;
    let mut has_template_content = false;

    loop {
        match iter.peek() {
            Some(Node::Text(text)) => {
                iter.next();
                builder_lines.push_str(&format!(
                    "    let builder = string_builder.append(builder, \"{}\")\n",
                    text.replace('\"', "\\\"")
                ));

                // We have some kind of content if the text is not only whitespace. We don't need
                // to handle this recursively as we're only interested in the top level.
                has_template_content = has_template_content || !text.trim().is_empty();
            }
            Some(Node::Identifier(name)) => {
                iter.next();
                builder_lines.push_str(&format!(
                    "    let builder = string_builder.append(builder, {})\n",
                    name
                ));
                has_template_content = true;
            }
            Some(Node::Builder(name)) => {
                iter.next();
                builder_lines.push_str(&format!(
                    "    let builder = string_builder.append_builder(builder, {})\n",
                    name
                ));
                has_template_content = true;
            }
            Some(Node::Import(import_details)) => {
                iter.next();
                imports.push(import_details.clone());
            }
            Some(Node::With((identifier, range), type_)) => {
                iter.next();

                if typed_params.iter().any(|(name, _)| name == identifier) {
                    return Err(RenderError::DuplicateParamName(
                        identifier.clone(),
                        range.clone(),
                    ));
                }

                typed_params.push((identifier.clone(), type_.clone()));
                has_template_content = true;
            }
            Some(Node::If(identifier_name, if_nodes, else_nodes)) => {
                iter.next();
                let if_context = render_lines(&mut if_nodes.iter().peekable())?;
                let else_context = render_lines(&mut else_nodes.iter().peekable())?;
                builder_lines.push_str(&format!(
                    r#"    let builder = case {} {{
        True -> {{
            {}
            builder
        }}
        False -> {{
            {}
            builder
        }}
}}
"#,
                    identifier_name, if_context.builder_lines, else_context.builder_lines
                ));
                includes_for_loop = includes_for_loop
                    || if_context.includes_for_loop
                    || else_context.includes_for_loop;
                has_template_content = true;
            }
            Some(Node::For(entry_identifier, entry_type, list_identifier, loop_nodes)) => {
                iter.next();

                let entry_type = entry_type
                    .as_ref()
                    .map(|value| format!(": {}", value))
                    .unwrap_or_else(|| "".to_string());

                let loop_context = render_lines(&mut loop_nodes.iter().peekable())?;
                builder_lines.push_str(&format!(
                    r#"    let builder = list.fold({}, builder, fn(builder, {}{}) {{
        {}
        builder
}})
"#,
                    list_identifier, entry_identifier, entry_type, loop_context.builder_lines
                ));

                includes_for_loop = true;
                has_template_content = true;
            }
            Some(Node::BlockFunction(visiblity, head, body_nodes, _range)) => {
                iter.next();
                let visibility_text = match visiblity {
                    Visibility::Private => "",
                    Visibility::Public => "pub ",
                };
                let body_context = render_lines(&mut body_nodes.iter().peekable())?;
                let body = body_context.builder_lines;
                functions.push(format!(
                    r#"{visibility_text}fn {head} -> StringBuilder {{
    let builder = string_builder.from_string("")
{body}
    builder
}}"#,
                ));

                includes_for_loop = includes_for_loop || body_context.includes_for_loop;
            }
            None => break,
        }
    }

    Ok(Context {
        builder_lines,
        imports,
        functions,
        typed_params,
        includes_for_loop,
        has_template_content,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::parser::{self, ParserError};
    use crate::scanner::{self, ScanError};

    #[derive(Debug)]
    pub enum Error {
        Scan(ScanError),
        Parse(ParserError),
        Render(RenderError),
    }

    fn format_result(result: Result<String, Error>) -> String {
        match result {
            Ok(value) => value,
            Err(err) => format!("{:?}", err),
        }
    }

    const NAME: &str = env!("CARGO_PKG_NAME");

    #[macro_export]
    macro_rules! assert_render {
        ($text:expr $(,)?) => {{
            let _ = env_logger::try_init();
            let result = scanner::scan($text)
                .map_err(|err| Error::Scan(err))
                .and_then(|tokens| {
                    parser::parse(&mut tokens.iter().peekable()).map_err(|err| Error::Parse(err))
                })
                .and_then(|ast| {
                    render(&mut ast.iter().peekable(), NAME, "-test-")
                        .map_err(|err| Error::Render(err))
                });
            insta::assert_snapshot!(insta::internals::AutoName, format_result(result), $text);
        }};
    }

    // Render

    #[test]
    fn test_render_pure_text() {
        assert_render!("Hello name, good to meet you");
    }

    #[test]
    fn test_render_identifier() {
        assert_render!(
            "{> with name as String
Hello {{ name }}, good to meet you"
        );
    }

    #[test]
    fn test_render_two_identifiers() {
        assert_render!(
            "{> with name as String
{> with adjective as String
Hello {{ name }}, {{ adjective }} to meet you"
        );
    }

    #[test]
    fn test_render_gleam_expression() {
        assert_render!(
            "{> import gleam/string
Hello {{ string.uppercase(name) }}, good to meet you"
        );
    }

    #[test]
    fn test_repeated_identifier_usage() {
        assert_render!(
            "{> with name as String
{{ name }} usage, {{ name }} usage"
        );
    }

    #[test]
    fn test_render_if_statement() {
        assert_render!(
            "{> with is_user as Bool
Hello {% if is_user %}User{% endif %}"
        );
    }

    #[test]
    fn test_render_empty_if_statement() {
        assert_render!(
            "{> with is_user as Bool
Hello {% if is_user %}{% endif %}"
        );
    }

    #[test]
    fn test_render_if_else_statement() {
        assert_render!(
            "{> with is_user as Bool
Hello {% if is_user %}User{% else %}Unknown{% endif %}"
        );
    }

    #[test]
    fn test_render_if_comparison() {
        assert_render!("Hello {% if items != [] %}Some items{% endif %}");
    }

    #[test]
    fn test_render_nested_if_statements() {
        assert_render!(
            "{> with is_user as Bool
{> with is_admin as Bool
Hello {% if is_user %}{% if is_admin %}Admin{% else %}User{% endif %}{% endif %}"
        );
    }

    #[test]
    fn test_render_for_loop() {
        assert_render!(
            "{> with list as List(String)
Hello,{% for item in list %} to {{ item }} and {% endfor %} everyone else"
        );
    }

    #[test]
    fn test_render_for_as_loop() {
        assert_render!(
            "{> with list as List(Item)
Hello,{% for item as Item in list %} to {{ item }} and {% endfor %} everyone else"
        );
    }

    #[test]
    fn test_render_for_from_expression() {
        assert_render!("Hello {% for item as Item in list.take(list, 2) %}{{ item }}{% endfor %}");
    }

    #[test]
    fn test_render_dot_access() {
        assert_render!(
            "{> with user as MyUser
Hello{% if user.is_admin %} Admin{% endif %}"
        );
    }

    #[test]
    fn test_render_import() {
        assert_render!("{> import user.{User}\n{> with name as String\n{{ name }}");
    }

    #[test]
    fn test_render_with() {
        assert_render!("{> with user as User\n{{ user }}");
    }

    #[test]
    fn test_render_import_and_with() {
        assert_render!("{> import user.{User}\n{> with user as User\n{{ user }}");
    }

    #[test]
    fn test_render_multiline() {
        assert_render!(
            r#"{> with my_list as List(String)
<ul>
{% for entry in my_list %}
    <li>{{ entry }}</li>
{% endfor %}
</ul>"#
        );
    }

    #[test]
    fn test_render_quotes() {
        assert_render!(
            r#"{> with name as String
<div class="my-class">{{ name }}</div>"#
        );
    }

    #[test]
    fn test_render_builder_block() {
        assert_render!(
            "{> with name as StringBuilder
Hello {[ name ]}, good to meet you"
        );
    }

    #[test]
    fn test_render_builder_expression() {
        assert_render!("Hello {[ string_builder.from_strings([\"Anna\", \" and \", \"Bob\"]) ]}, good to meet you");
    }

    #[test]
    fn test_render_function() {
        assert_render!("{> fn classes()\na b c d\n{> endfn\nHello world");
    }

    #[test]
    fn test_render_public_function() {
        assert_render!("{> pub fn classes()\na b c d\n{> endfn\nHello world");
    }

    #[test]
    fn test_render_only_public_functions() {
        assert_render!(
            r#"
{> pub fn classes()
    a b c d
{> endfn

{> pub fn item(name: String)
    <li class="item">{{ name }}</li>
{> endfn
"#
        );
    }

    #[test]
    fn test_render_function_and_usage() {
        assert_render!("{> fn name()\nLucy\n{> endfn\nHello {[ name() ]}");
    }

    #[test]
    fn test_render_function_with_arg_and_usage() {
        assert_render!(
            r#"{> fn full_name(second_name: String)
Lucy {{ second_name }}
{> endfn
Hello {[ full_name("Gleam") ]}"#
        );
    }

    #[test]
    fn test_render_function_with_for_loop() {
        assert_render!(
            r#"{> fn full_name(names: List(String))
{% for name in names %}{{ name }},{% endfor %}"
{> endfn
Hello {[ names("Gleam") ]}"#
        );
    }
}
