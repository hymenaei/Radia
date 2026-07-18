uniform mat4 modelview_projection_matrix;

in vec3 position;
in vec4 diffuse_color;
in vec2 texcoord0;

out vec4 vertex_color;
out vec2 shape_coord;

void main()
{
    gl_Position = modelview_projection_matrix * vec4(position, 1.0);
    vertex_color = diffuse_color;
    shape_coord = texcoord0;
}
