
pub struct ParsedCommand<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

pub fn parse(input: &str) -> Option<ParsedCommand<'_>> {
    let input = input.trim();

    if input.is_empty() {
        return None;
    }

    let split_position = input
        .find(char::is_whitespace);

    match split_position {
        Some(position) => {
            let name = &input[..position];
            let arguments = input[position..].trim();

            Some(ParsedCommand {
                name,
                arguments,
            })
        }

        None => Some(ParsedCommand {
            name: input,
            arguments: "",
        }),
    }
}
