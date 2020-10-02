use anyhow::*;
use rayon::prelude::*;
use std::ops::Range;
use std::path::Path;
use wgpu::util::DeviceExt;

use crate::texture;
use crate::pipeline;

pub trait Vertex {
    fn desc<'a>() -> wgpu::VertexBufferDescriptor<'a>;
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ModelVertex {
    position: cgmath::Vector3<f32>,
    tex_coords: cgmath::Vector2<f32>,
    normal: cgmath::Vector3<f32>,
    tangent: cgmath::Vector3<f32>,
    bitangent: cgmath::Vector3<f32>,
}

unsafe impl bytemuck::Zeroable for ModelVertex {}
unsafe impl bytemuck::Pod for ModelVertex {}

impl Vertex for ModelVertex {
    fn desc<'a>() -> wgpu::VertexBufferDescriptor<'a> {
        use std::mem;
        wgpu::VertexBufferDescriptor {
            stride: mem::size_of::<ModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::InputStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttributeDescriptor {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float3,
                },
                wgpu::VertexAttributeDescriptor {
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float2,
                },
                wgpu::VertexAttributeDescriptor {
                    offset: mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float3,
                },
                // Tangent and bitangent
                wgpu::VertexAttributeDescriptor {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float3,
                },
                wgpu::VertexAttributeDescriptor {
                    offset: mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float3,
                },
            ],
        }
    }
}

pub struct Material {
    pub name: String,
    pub diffuse_texture: texture::Texture,
    pub normal_texture: texture::Texture,
    pub bind_group: wgpu::BindGroup,
}

impl Material {
    pub fn new(
        device: &wgpu::Device,
        name: &str,
        diffuse_texture: texture::Texture,
        normal_texture: texture::Texture,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&diffuse_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&diffuse_texture.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&normal_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&normal_texture.sampler),
                },
            ],
            label: Some(name),
        });

        Self {
            name: String::from(name),
            diffuse_texture,
            normal_texture,
            bind_group,
        }
    }
}

pub struct Mesh {
    pub name: String,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_elements: u32,
    pub material: usize,
}

impl pipeline::Bindable for Mesh {
    fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
        vec![
            // Vertices
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStage::COMPUTE,
                ty: wgpu::BindingType::StorageBuffer {
                    dynamic: false,
                    min_binding_size: None,
                    // We WILL change the vertices in the compute shader
                    readonly: false,
                },
                count: None,
            },
            // Indices
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStage::COMPUTE,
                ty: wgpu::BindingType::StorageBuffer {
                    dynamic: false,
                    min_binding_size: None,
                    // We WILL NOT change the indices in the compute shader
                    readonly: true,
                },
                count: None,
            },
        ]
    }

    fn bind_group_entries(&self) -> Vec<wgpu::BindGroupEntry> {
        vec![
            // Vertices
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(self.vertex_buffer.slice(..)),
            },
            // Indices
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(self.index_buffer.slice(..)),
            },
        ]
    }
}

pub struct Model {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
}

pub struct ModelLoader {
    binder: pipeline::Binder<Mesh>,
    pipeline: wgpu::ComputePipeline,
}

// UPDATED!
impl ModelLoader {

    // NEW!
    pub fn new(device: &wgpu::Device) -> Self {
        let binder = pipeline::Binder::new(device, Some("ModelLoader Binder"));
        let shader_src = wgpu::include_spirv!("model_load.comp.spv");
        let pipeline = pipeline::create_compute_pipeline(device, &[&binder.layout], shader_src, Some("ModelLoader ComputePipeline"));
        Self { binder, pipeline }
    }

    // UPDATED!
    pub fn load<P: AsRef<Path>>(
        &self, // NEW!
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        path: P,
    ) -> Result<Model> {
        let (obj_models, obj_materials) = tobj::load_obj(path.as_ref(), true)?;

        // We're assuming that the texture files are stored with the obj file
        let containing_folder = path.as_ref().parent().context("Directory has no parent")?;

        let materials = obj_materials
            .par_iter()
            .map(|mat| {
                // We can also parallelize loading the textures!
                let mut textures = [
                    (containing_folder.join(&mat.diffuse_texture), false),
                    (containing_folder.join(&mat.normal_texture), true),
                ]
                .par_iter()
                .map(|(texture_path, is_normal_map)| {
                    texture::Texture::load(device, queue, texture_path, *is_normal_map)
                })
                .collect::<Result<Vec<_>>>()?;

                // Pop removes from the end of the list.
                let normal_texture = textures.pop().unwrap();
                let diffuse_texture = textures.pop().unwrap();

                Ok(Material::new(
                    device,
                    &mat.name,
                    diffuse_texture,
                    normal_texture,
                    layout,
                ))
            })
            .collect::<Result<Vec<Material>>>()?;

        let meshes = obj_models
            .par_iter()
            .map(|m| {
                let vertices = (0..m.mesh.positions.len() / 3)
                    .into_par_iter()
                    .map(|i| {
                        ModelVertex {
                            position: [
                                m.mesh.positions[i * 3],
                                m.mesh.positions[i * 3 + 1],
                                m.mesh.positions[i * 3 + 2],
                            ]
                            .into(),
                            tex_coords: [m.mesh.texcoords[i * 2], m.mesh.texcoords[i * 2 + 1]]
                                .into(),
                            normal: [
                                m.mesh.normals[i * 3],
                                m.mesh.normals[i * 3 + 1],
                                m.mesh.normals[i * 3 + 2],
                            ]
                            .into(),
                            // We'll calculate these later
                            tangent: [0.0; 3].into(),
                            bitangent: [0.0; 3].into(),
                        }
                    })
                    .collect::<Vec<_>>();

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{:?} Vertex Buffer", m.name)),
                    contents: bytemuck::cast_slice(&vertices),
                    // UPDATED!
                    usage: wgpu::BufferUsage::VERTEX | wgpu::BufferUsage::STORAGE,
                });
                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{:?} Index Buffer", m.name)),
                    contents: bytemuck::cast_slice(&m.mesh.indices),
                    // UPDATED!
                    usage: wgpu::BufferUsage::INDEX | wgpu::BufferUsage::STORAGE,
                });

                // NEW!
                // We'll need the mesh for the tangent/bitangent calculation
                let mesh = Mesh {
                    name: m.name.clone(),
                    vertex_buffer,
                    index_buffer,
                    num_elements: m.mesh.indices.len() as u32,
                    material: m.mesh.material_id.unwrap_or(0),
                };

                // Calculate the tangents and bitangents
                let calc_bind_group = self.binder.create_bind_group(
                    &mesh, 
                    device, 
                    Some("Mesh BindGroup")
                );
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Tangent and Bitangent Calc"),
                });
                {
                    let mut pass = encoder.begin_compute_pass();
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &calc_bind_group, &[]);
                    pass.dispatch(mesh.num_elements as u32 / 3, 1, 1);
                }
                queue.submit(std::iter::once(encoder.finish()));
                device.poll(wgpu::Maintain::Wait);

                Ok(mesh)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Model { meshes, materials })
    }
}

pub trait DrawModel<'a, 'b>
where
    'b: 'a,
{
    fn draw_mesh(
        &mut self,
        mesh: &'b Mesh,
        material: &'b Material,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
    fn draw_mesh_instanced(
        &mut self,
        mesh: &'b Mesh,
        material: &'b Material,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );

    fn draw_model(
        &mut self,
        model: &'b Model,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
    fn draw_model_instanced(
        &mut self,
        model: &'b Model,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
    fn draw_model_instanced_with_material(
        &mut self,
        model: &'b Model,
        material: &'b Material,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
}

impl<'a, 'b> DrawModel<'a, 'b> for wgpu::RenderPass<'a>
where
    'b: 'a,
{
    fn draw_mesh(
        &mut self,
        mesh: &'b Mesh,
        material: &'b Material,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.draw_mesh_instanced(mesh, material, 0..1, uniforms, light);
    }

    fn draw_mesh_instanced(
        &mut self,
        mesh: &'b Mesh,
        material: &'b Material,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        self.set_index_buffer(mesh.index_buffer.slice(..));
        self.set_bind_group(0, &material.bind_group, &[]);
        self.set_bind_group(1, &uniforms, &[]);
        self.set_bind_group(2, &light, &[]);
        self.draw_indexed(0..mesh.num_elements, 0, instances);
    }

    fn draw_model(
        &mut self,
        model: &'b Model,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.draw_model_instanced(model, 0..1, uniforms, light);
    }

    fn draw_model_instanced(
        &mut self,
        model: &'b Model,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        for mesh in &model.meshes {
            let material = &model.materials[mesh.material];
            self.draw_mesh_instanced(mesh, material, instances.clone(), uniforms, light);
        }
    }

    fn draw_model_instanced_with_material(
        &mut self,
        model: &'b Model,
        material: &'b Material,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        for mesh in &model.meshes {
            self.draw_mesh_instanced(mesh, material, instances.clone(), uniforms, light);
        }
    }
}

pub trait DrawLight<'a, 'b>
where
    'b: 'a,
{
    fn draw_light_mesh(
        &mut self,
        mesh: &'b Mesh,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
    fn draw_light_mesh_instanced(
        &mut self,
        mesh: &'b Mesh,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) where
        'b: 'a;

    fn draw_light_model(
        &mut self,
        model: &'b Model,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
    fn draw_light_model_instanced(
        &mut self,
        model: &'b Model,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    );
}

impl<'a, 'b> DrawLight<'a, 'b> for wgpu::RenderPass<'a>
where
    'b: 'a,
{
    fn draw_light_mesh(
        &mut self,
        mesh: &'b Mesh,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.draw_light_mesh_instanced(mesh, 0..1, uniforms, light);
    }

    fn draw_light_mesh_instanced(
        &mut self,
        mesh: &'b Mesh,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        self.set_index_buffer(mesh.index_buffer.slice(..));
        self.set_bind_group(0, uniforms, &[]);
        self.set_bind_group(1, light, &[]);
        self.draw_indexed(0..mesh.num_elements, 0, instances);
    }

    fn draw_light_model(
        &mut self,
        model: &'b Model,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        self.draw_light_model_instanced(model, 0..1, uniforms, light);
    }
    fn draw_light_model_instanced(
        &mut self,
        model: &'b Model,
        instances: Range<u32>,
        uniforms: &'b wgpu::BindGroup,
        light: &'b wgpu::BindGroup,
    ) {
        for mesh in &model.meshes {
            self.draw_light_mesh_instanced(mesh, instances.clone(), uniforms, light);
        }
    }
}