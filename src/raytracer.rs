use anyhow::Result;
use itertools::multiunzip;
use sdl_wrapper::ScreenContextManager;
use std::path::Path;

use crate::constants::{MAX_REFLECTIONS, SHADOWS, TOLERANCE};
use crate::scene::{Light, Observer, Scene};
use crate::shapes::{colors::BLACK, Color, Shape, ShapeCalculations};
use crate::vec3::Vec3;

#[derive(Debug)]
pub struct Ray {
    pub anchor: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn from_2_points(p_origin: Vec3, p_target: Vec3) -> Ray {
        // println!("Ray from origin: {:?}\tto target: {:?}", p_origin, p_target);
        // println!("target - og: {:?}", p_target - p_origin);
        let v_dir: Vec3 = (p_target - p_origin).normalize();
        Ray {
            anchor: p_origin,
            dir: v_dir,
        }
    }
}

impl Ray {
    pub fn point_at_t(&self, t: f64) -> Vec3 {
        self.anchor + self.dir * t
    }
}

pub fn raytrace<P: AsRef<Path>>(
    path: P,
    observer: &Observer,
    scene: &Scene,
    screen: &mut ScreenContextManager,
) -> Result<()> {
    let ratio_x = (observer.max_p.x - observer.min_p.x) / f64::from(screen.get_width());
    let ratio_y = (observer.max_p.y - observer.min_p.y) / f64::from(screen.get_height());

    let z_t = observer.min_p.z;

    let height = screen.get_height();
    let update_interval = screen.get_width() / 10;

    for i in 0..screen.get_width() {
        for j in 0..screen.get_height() {
            // Get ray
            let x_t = (f64::from(i) + 0.5) * ratio_x + observer.min_p.x;
            let y_t = (f64::from(j) + 0.5) * ratio_y + observer.min_p.y;
            let target = Vec3::new(x_t, y_t, z_t);
            let ray = Ray::from_2_points(observer.camera, target);

            // Get color
            let color = get_color_pixel(ray, scene, 1.0, MAX_REFLECTIONS);

            // Paint
            screen.set_color(color.r as f32, color.g as f32, color.b as f32);
            screen.plot_pixel(i, (height - 1) - j); // flip images so they're not upside down
        }
        if i % update_interval == 0 {
            screen.present()?;
        }
    }

    screen.present()?;

    screen.save_img(path)?;

    Ok(())
}

/// o1 = percentage of color that belongs to the current call (relevant for reflections and
/// transparencies)
fn get_color_pixel(ray: Ray, scene: &Scene, total_o1: f64, reflections: u32) -> Color {
    if let Some(inter) = get_first_intersection(&ray, scene) {
        let normal = inter.object.get_normal_vec(inter.point);

        let backwards_vec = -1.0 * ray.dir;

        // Calculate stuff relating to each specific light that has to be reused, for optimization
        // purposes
        let (shadow_intersections, light_factors, l_vecs): (
            Vec<Option<Intersection>>,
            Vec<f64>,
            Vec<Vec3>,
        ) = multiunzip(scene.get_lights().iter().map(|light| {
            (
                if SHADOWS {
                    get_shadow_intersection(
                        &Ray::from_2_points(inter.point, light.position),
                        scene,
                        light,
                    )
                } else {
                    None
                },
                // F_att * Ip
                light.get_attenuation((light.position - inter.point).norm()) * light.intensity,
                // L vectors
                light.get_l_vec(inter.point),
            )
        }));

        let total_intensity = (scene
            .get_lights()
            .iter()
            .enumerate()
            .map(|(i, light)| {
                if !SHADOWS || shadow_intersections[i].is_none() {
                    let intensity: f64 =
                        (l_vecs[i].dot(normal)).max(0.0) * light_factors[i] * inter.object.k_d();

                    light.color * intensity
                } else {
                    BLACK
                }
            })
            .sum::<Color>()
            + (scene.ambient_color * scene.ambient * inter.object.k_a()))
        .min(1.0);

        let rgb_d = total_intensity * inter.object.get_color();

        let total_speculation = (scene
            .get_lights()
            .iter()
            .enumerate()
            .map(|(i, light)| {
                if !SHADOWS
                    || get_shadow_intersection(
                        &Ray::from_2_points(inter.point, light.position),
                        scene,
                        light,
                    )
                    .is_none()
                {
                    let reflection_vec: Vec3 = 2.0 * normal * (normal.dot(l_vecs[i])) - l_vecs[i];

                    let specular: f64 = (reflection_vec.dot(backwards_vec))
                        .max(0.0)
                        .powf(inter.object.k_n())
                        * light_factors[i]
                        * inter.object.k_s();

                    (light.color - rgb_d) * specular
                } else {
                    BLACK
                }
            })
            .sum::<Color>())
        .min(1.0);

        let o1 = inter.object.o1();
        if o1 < 1.0 && total_o1 > TOLERANCE && reflections > 0 {
            let reflection_dir = ray.dir - 2.0 * (ray.dir.dot(normal)) * normal;

            // We advance the anchor a bit (a TOLERANCE amount) to avoid the sphere getting stuck
            // reflecting itself due to float rounding error
            let reflection_vec = Ray {
                anchor: inter.point + TOLERANCE * reflection_dir,
                dir: reflection_dir,
            };
            let reflection = inter.object.reflection();
            o1 * (rgb_d + total_speculation)
                + reflection
                    * get_color_pixel(
                        reflection_vec,
                        scene,
                        total_o1 * reflection,
                        reflections - 1,
                    )
        } else {
            rgb_d + total_speculation
        }
    } else {
        scene.bg_color
    }
}

struct Intersection<'a> {
    //t: f64,
    object: &'a Shape,
    point: Vec3,
}

fn get_first_intersection<'a>(ray: &Ray, scene: &'a Scene) -> Option<Intersection<'a>> {
    // Init tmin and the intersected shape
    let mut tmin = f64::INFINITY;
    let mut intersection: Option<Intersection> = None;

    for object in scene.get_objects() {
        if let Some(t) = object.get_intersection(ray) {
            if t < tmin {
                tmin = t;
                intersection = Some(Intersection {
                    //t: tmin,
                    object,
                    point: ray.point_at_t(tmin),
                });
            }
        }
    }

    intersection
}

fn get_shadow_intersection<'a>(
    ray: &Ray,
    scene: &'a Scene,
    light: &Light,
) -> Option<Intersection<'a>> {
    let t_light: f64 = (light.position - ray.anchor).norm();

    let mut intersection: Option<Intersection> = None;

    for object in scene.get_objects() {
        if let Some(t) = object.get_intersection(ray) {
            if t < t_light && t > TOLERANCE {
                // revisamos t > TOLERANCE para que el objeto no se auto-detecte como intersección
                intersection = Some(Intersection {
                    //t,
                    object,
                    point: ray.point_at_t(t),
                });
                return intersection;
            }
        }
    }

    intersection
}
