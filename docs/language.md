# Parameters language reference

Parameters workflows are plain text files with the `.parameters` extension. Each file is an exact view of a JSON workflow document: `parameters convert` turns one into the other without losing anything except graph positions, which only the JSON keeps.

Statements go one per line. Settings and fields belong to the statement above them and are indented by two spaces. There are no comments; use `note` instead, so the description survives in the JSON and the graph.

## Example

```parameters
workflow "Deploy site to Netlify"
  note "Builds the site and deploys it when main changes"
  concurrency 4

input branch: text = "main"
input site_dir: path = "site"

step checkout uses process.run
  program is "git"
  arguments is ["clone", "--branch", branch, "https://github.com/acme/site", site_dir]

step test uses shell.run
  after checkout
  command is "npm test"
  directory is site_dir

step lint uses shell.run
  after checkout
  command is "npm run lint"
  directory is site_dir

step deploy uses netlify.deploy
  after test and lint
  retry 3 times
  timeout 10m
  site is secret "NETLIFY_SITE_ID"
  token is secret "NETLIFY_TOKEN"
  folder is "{site_dir}/dist"

step notify uses http.request
  when deploy succeeded
  method is "POST"
  url is secret "SLACK_WEBHOOK_URL"
  body is { text: "Deployed {deploy.url}" }

output url is deploy.url

define node netlify.deploy
  note "Uploads a folder to a Netlify site"
  input site: secret
  input token: secret
  input folder: path

  step upload uses http.request
    method is "POST"
    url is "https://api.netlify.com/api/v1/sites/{site}/deploys"
    headers is { Authorization: "Bearer {token}" }
    file is folder

  output url: text is upload.body.deploy_url
```

## Statements

| Statement | Where | Meaning |
| --- | --- | --- |
| `workflow "Name"` | first line | Names the workflow. `note` and `concurrency` can be indented below it. |
| `note "Text"` | under `workflow`, `input`, `step` or `define node` | A description that is kept in the JSON and shown in the graph. |
| `concurrency 4` | under `workflow` | The most steps that may run at the same time. Steps run in parallel unless something orders them. |
| `input name: type = value` | top level or `define node` | A value given when the workflow runs. The default after `=` is optional and must be a plain value. |
| `step id uses node.type` | top level or `define node` | Runs a node. Its settings and fields are indented below it. |
| `after a, b and c` | under `step` | Waits for these steps to finish first. Referring to another step's output also waits for it. |
| `when condition` | under `step` | Runs the step only when the condition is true; otherwise it is skipped. |
| `retry 3 times` | under `step` | Retries a failing step, with growing pauses between attempts. |
| `timeout 30s` | under `step` | Fails the step if it runs longer. Units: `ms`, `s`, `m`, `h`. |
| `field is value` | under `step` | Sets one of the node's fields to a value or expression. |
| `output name: type is value` | top level or `define node` | A result. The `: type` part is optional. |
| `define node namespace.name` | top level | A custom node: a small workflow with inputs and outputs that other steps can use. |

## Types

| Type | Values |
| --- | --- |
| `text` | Text in double quotes, such as `"main"`. |
| `number` | A whole or decimal number, such as `3` or `-2.5`. |
| `boolean` | `true` or `false`. |
| `list` | Values in square brackets, such as `[1, 2, 3]`. |
| `object` | Named values in braces, such as `{ name: "x", size: 2 }`. |
| `path` | A file or folder path, written as text. |
| `secret` | A stored secret, written as `secret "NAME"`. Secret values are never stored in the workflow. |
| `any` | Any value. |

## Expressions

Values after `is`, `when` and `=` are expressions:

- **Plain values:** text, numbers, `true`, `false`, lists `[a, b]` and objects `{ key: value }`. Object keys that are not simple words go in quotes: `{ "Content-Type": "json" }`.
- **References:** an input by name (`branch`), a step's output (`checkout.path`) or a field inside it (`upload.body.deploy_url`).
- **Interpolation:** references in braces inside text, such as `"{site_dir}/dist"`. Write `\{` and `\}` for literal braces.
- **Escapes in text:** `\"`, `\\`, `\n`, `\t`, `\r`, `\{`, `\}` and `\u{1F600}`.
- **Secrets:** `secret "NAME"` reads a stored secret by name.
- **Step state:** `deploy succeeded`, `deploy failed` and `deploy skipped`.
- **Comparisons:** `==`, `!=`, `<`, `<=`, `>` and `>=`. Comparisons cannot be chained.
- **Logic:** `not`, then `and`, then `or`, from tightest to loosest. Use parentheses to group differently.

An expression that only contains plain values is stored as plain JSON; anything else is stored as `{"$expr": "..."}`. Long lists and objects may continue onto the next lines until their brackets close.

## Reserved keywords

These words cannot be used as names of steps, inputs, outputs or fields:

`workflow` `define` `node` `step` `uses` `input` `output` `note` `is` `secret` `true` `false` `after` `when` `and` `or` `not` `succeeded` `failed` `skipped` `retry` `times` `timeout` `concurrency` `for` `each` `in` `import` `as` `from`

## Diagnostics

| Code | Severity | Meaning |
| --- | --- | --- |
| `P0001` | error | invalid syntax |
| `P0002` | error | invalid indentation |
| `P0003` | error | duplicate statement |
| `P0101` | error | invalid name |
| `P0102` | error | reserved keyword used as a name |
| `P0103` | error | invalid node type |
| `P0104` | error | duplicate name |
| `P0201` | error | unknown node type |
| `P0202` | error | built-in node redefined |
| `P0203` | error | unknown field |
| `P0204` | error | missing required field |
| `P0205` | error | type mismatch |
| `P0206` | error | secret written as plain text |
| `P0207` | error | reserved `$` key |
| `P0208` | error | invalid setting |
| `P0301` | error | invalid expression |
| `P0302` | error | unknown reference |
| `P0303` | error | unknown output |
| `P0304` | error | field access on a value without fields |
| `P0305` | error | unknown step |
| `P0401` | error | dependency cycle |
| `P0402` | error | custom nodes that use each other |
| `P0501` | warning | layout entry for a missing step |
| `P0502` | error | limit exceeded |
