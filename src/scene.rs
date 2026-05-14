//! A self-contained procedural scene — no external asset files.
//!
//! The layout is a loose open box (floor + three coloured walls) holding a few
//! blocks, a mirror-like metallic sphere, and a bright emissive ceiling slab.
//! That gives Solari plenty of bounce lighting and sharp reflections — exactly
//! the noisy signal DLSS Ray Reconstruction is built to denoise.
//!
//! Every mesh gets a [`RaytracingMesh3d`] (the BLAS Solari traces against). In
//! realtime mode it *also* gets a [`Mesh3d`] so it is rasterised for primary
//! visibility; the pathtracer skips rasterisation, so `Mesh3d` is left off.

use bevy::{prelude::*, solari::prelude::RaytracingMesh3d};

use crate::AppArgs;

pub fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    args: Res<AppArgs>,
) {
    let rasterize = !args.pathtracer;

    // Solari requires UV_0 + TANGENT + 32-bit indices. Primitive builders emit
    // U32 indices already; `with_generated_tangents` adds UV_0 and TANGENT.
    let plane = meshes.add(
        Plane3d::default()
            .mesh()
            .size(44.0, 44.0)
            .build()
            .with_generated_tangents()
            .unwrap(),
    );
    let cube = meshes.add(
        Cuboid::default()
            .mesh()
            .build()
            .with_generated_tangents()
            .unwrap(),
    );
    let sphere = meshes.add(
        Sphere::new(2.0)
            .mesh()
            .build()
            .with_generated_tangents()
            .unwrap(),
    );

    let white = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.8, 0.8),
        perceptual_roughness: 0.6,
        ..default()
    });
    let red = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.15, 0.15),
        perceptual_roughness: 0.7,
        ..default()
    });
    let green = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.7, 0.25),
        perceptual_roughness: 0.7,
        ..default()
    });
    let glossy = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.35, 0.85),
        perceptual_roughness: 0.25,
        ..default()
    });
    // Near-mirror metal — its reflections are where DLSS-RR earns its keep.
    let metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.95, 1.0),
        metallic: 1.0,
        perceptual_roughness: 0.06,
        ..default()
    });
    // Bright emissive slab: the area light Solari bounces around the room.
    let lamp = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: LinearRgba::rgb(1.0, 0.92, 0.78) * 50_000.0,
        ..default()
    });

    // Floor.
    spawn_mesh(&mut commands, &plane, white.clone(), Transform::IDENTITY, rasterize);

    // Three walls — a flattened unit cube each — to give GI surfaces to bounce off.
    spawn_mesh(
        &mut commands,
        &cube,
        white.clone(),
        Transform::from_xyz(0.0, 11.0, -22.0).with_scale(Vec3::new(44.0, 22.0, 0.5)),
        rasterize,
    );
    spawn_mesh(
        &mut commands,
        &cube,
        red,
        Transform::from_xyz(-22.0, 11.0, 0.0).with_scale(Vec3::new(0.5, 22.0, 44.0)),
        rasterize,
    );
    spawn_mesh(
        &mut commands,
        &cube,
        green,
        Transform::from_xyz(22.0, 11.0, 0.0).with_scale(Vec3::new(0.5, 22.0, 44.0)),
        rasterize,
    );

    // Two blocks.
    spawn_mesh(
        &mut commands,
        &cube,
        white.clone(),
        Transform::from_xyz(-7.0, 5.0, -7.0)
            .with_scale(Vec3::new(7.0, 10.0, 7.0))
            .with_rotation(Quat::from_rotation_y(0.4)),
        rasterize,
    );
    spawn_mesh(
        &mut commands,
        &cube,
        glossy,
        Transform::from_xyz(6.0, 3.0, 3.0)
            .with_scale(Vec3::splat(6.0))
            .with_rotation(Quat::from_rotation_y(-0.3)),
        rasterize,
    );

    // Metallic sphere (radius 2.0, sat on the floor).
    spawn_mesh(
        &mut commands,
        &sphere,
        metal,
        Transform::from_xyz(8.0, 2.0, -6.0),
        rasterize,
    );

    // Emissive ceiling slab.
    spawn_mesh(
        &mut commands,
        &cube,
        lamp,
        Transform::from_xyz(0.0, 21.0, 0.0).with_scale(Vec3::new(16.0, 0.5, 16.0)),
        rasterize,
    );

    // Sun. Solari does its own raytraced shadows, so shadow maps stay off.
    commands.spawn((
        DirectionalLight {
            illuminance: light_consts::lux::FULL_DAYLIGHT,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0).looking_to(Vec3::new(-0.4, -1.0, -0.35), Vec3::Y),
    ));
}

/// Spawn one mesh instance: always raytraced, additionally rasterised when not
/// in pathtracer mode.
fn spawn_mesh(
    commands: &mut Commands,
    mesh: &Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
    rasterize: bool,
) {
    let mut entity = commands.spawn((
        RaytracingMesh3d(mesh.clone()),
        MeshMaterial3d(material),
        transform,
    ));
    if rasterize {
        entity.insert(Mesh3d(mesh.clone()));
    }
}
