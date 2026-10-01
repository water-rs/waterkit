::shaderloom::CompiledShader::new(
    "src/yuv_to_rgba.wgsl",
    include_str!(
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders/compiled/yuv_color.wgsl")
    ),
    ::shaderloom::include_packaged_spirv!("src/shaders/compiled/yuv_color.spv"),
    ::shaderloom::include_compiled_metallib!("yuv_color.metallib"),
    &[
        ::shaderloom::CompiledEntryPoint {
            name: "convert_to_linear_rgba",
            stage: ::shaderloom::ShaderStage::Compute,
            metal_name: "convert_to_linear_rgba",
            workgroup_size: (8, 8, 1),
            dxil: ::shaderloom::include_compiled_dxil!(
                "yuv_color_compute_convert_to_linear_rgba.dxil"
            ),
        },
    ],
    &[
        ::shaderloom::ReflectedBindGroup {
            entries: &[
                ::shaderloom::wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ::shaderloom::wgpu::ShaderStages::from_bits_retain(4),
                    ty: ::shaderloom::wgpu::BindingType::Texture {
                        sample_type: ::shaderloom::wgpu::TextureSampleType::Uint,
                        view_dimension: ::shaderloom::wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                ::shaderloom::wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ::shaderloom::wgpu::ShaderStages::from_bits_retain(4),
                    ty: ::shaderloom::wgpu::BindingType::Texture {
                        sample_type: ::shaderloom::wgpu::TextureSampleType::Uint,
                        view_dimension: ::shaderloom::wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                ::shaderloom::wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ::shaderloom::wgpu::ShaderStages::from_bits_retain(4),
                    ty: ::shaderloom::wgpu::BindingType::Buffer {
                        ty: ::shaderloom::wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                ::shaderloom::wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: ::shaderloom::wgpu::ShaderStages::from_bits_retain(4),
                    ty: ::shaderloom::wgpu::BindingType::StorageTexture {
                        access: ::shaderloom::wgpu::StorageTextureAccess::WriteOnly,
                        format: ::shaderloom::wgpu::TextureFormat::Rgba16Float,
                        view_dimension: ::shaderloom::wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        },
    ],
)